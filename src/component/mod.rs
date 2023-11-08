use crate::core::{Custom, Export, FuncIdx, FuncType, Import, MemIdx, Module, TypeRef, ValType};
use crate::{IndexSpace, Section, SectionType, Sections};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};

#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "writer")]
pub mod writer;

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentSection<Expr: Debug + Clone + PartialEq + 'static> {
    Module(Module<Expr>),
    CoreInstance(Instance),
    CoreType(CoreType),
    Component(Component<Expr>),
    Instance(ComponentInstance),
    Alias(Alias),
    Type(ComponentType),
    Canon(Canon),
    Start(ComponentStart),
    Import(ComponentImport),
    Export(ComponentExport),
    Custom(Custom),
}

impl<Expr: Debug + Clone + PartialEq> Section<ComponentIndexSpace, ComponentSectionType>
    for ComponentSection<Expr>
{
    fn index_space(&self) -> ComponentIndexSpace {
        match self {
            ComponentSection::Module(module) => module.index_space(),
            ComponentSection::CoreInstance(core_instance) => core_instance.index_space(),
            ComponentSection::CoreType(core_type) => core_type.index_space(),
            ComponentSection::Component(component) => component.index_space(),
            ComponentSection::Instance(component_instance) => component_instance.index_space(),
            ComponentSection::Alias(alias) => alias.index_space(),
            ComponentSection::Type(component_type) => component_type.index_space(),
            ComponentSection::Canon(canon) => canon.index_space(),
            ComponentSection::Start(start) => start.index_space(),
            ComponentSection::Import(import) => import.index_space(),
            ComponentSection::Export(export) => export.index_space(),
            ComponentSection::Custom(custom) => custom.index_space(),
        }
    }

    fn section_type(&self) -> ComponentSectionType {
        match self {
            ComponentSection::Module(module) => module.section_type(),
            ComponentSection::CoreInstance(core_instance) => core_instance.section_type(),
            ComponentSection::CoreType(core_type) => core_type.section_type(),
            ComponentSection::Component(component) => component.section_type(),
            ComponentSection::Instance(component_instance) => component_instance.section_type(),
            ComponentSection::Alias(alias) => alias.section_type(),
            ComponentSection::Type(component_type) => component_type.section_type(),
            ComponentSection::Canon(canon) => canon.section_type(),
            ComponentSection::Start(start) => start.section_type(),
            ComponentSection::Import(import) => import.section_type(),
            ComponentSection::Export(export) => export.section_type(),
            ComponentSection::Custom(custom) => custom.section_type(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComponentSectionType {
    Module,
    CoreInstance,
    CoreType,
    Component,
    Instance,
    Alias,
    Type,
    Canon,
    Start,
    Import,
    Export,
    Custom,
}

impl SectionType for ComponentSectionType {
    fn allow_grouping(&self) -> bool {
        !matches!(
            self,
            ComponentSectionType::Module
                | ComponentSectionType::Component
                | ComponentSectionType::Start
                | ComponentSectionType::Custom
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComponentIndexSpace {
    Func,
    CoreType,
    Type,
    Module,
    Component,
    CoreInstance,
    Instance,
    Value,
    CoreTable,
    CoreFunc,
    CoreGlobal,
    CoreMem,
    Start,
    Custom,
}

impl IndexSpace for ComponentIndexSpace {
    type Index = u32;
}

type ComponentFuncIdx = u32;
type ComponentTypeIdx = u32;
type ModuleIdx = u32;
type ComponentIdx = u32;

#[allow(unused)]
type CoreInstanceIdx = u32;
type InstanceIdx = u32;
type ValueIdx = u32;

#[allow(unused)]
type StartIdx = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstantiationArgRef {
    Instance(InstanceIdx),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstantiationArg {
    pub name: String,
    pub arg_ref: InstantiationArgRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instance {
    Instantiate {
        module_idx: ModuleIdx,
        args: Vec<InstantiationArg>,
    },
    FromExports {
        exports: Vec<Export>,
    },
}

impl Section<ComponentIndexSpace, ComponentSectionType> for Instance {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::CoreInstance
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::CoreInstance
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentExternalKind {
    Module,
    Func,
    Value,
    Type,
    Instance,
    Component,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OuterAliasKind {
    CoreModule,
    CoreType,
    Type,
    Component,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportKind {
    Func,
    Table,
    Mem,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentInstantiationArg {
    pub name: String,
    pub kind: ComponentExternalKind,
    pub idx: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentInstance {
    Instantiate {
        component_idx: ComponentIdx,
        args: Vec<ComponentInstantiationArg>,
    },
    FromExports {
        exports: Vec<ComponentExport>,
    },
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentInstance {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Instance
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Instance
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentExternName {
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimitiveValueType {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Chr,
    Str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentValType {
    Primitive(PrimitiveValueType),
    Defined(ComponentTypeIdx),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeBounds {
    Eq(ComponentTypeIdx),
    SubResource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternDesc {
    // TODO: Rename to ComponentTypeRef
    Module(ComponentTypeIdx),
    Func(ComponentTypeIdx),
    Val(ComponentValType),
    Type(TypeBounds),
    Instance(ComponentTypeIdx),
    Component(ComponentTypeIdx),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentExport {
    pub name: ComponentExternName,
    pub kind: ComponentExternalKind,
    pub idx: u32,
    pub desc: Option<ExternDesc>,
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentExport {
    fn index_space(&self) -> ComponentIndexSpace {
        match self.kind {
            ComponentExternalKind::Module => ComponentIndexSpace::Module,
            ComponentExternalKind::Func => ComponentIndexSpace::Func,
            ComponentExternalKind::Value => ComponentIndexSpace::Value,
            ComponentExternalKind::Type => ComponentIndexSpace::Type,
            ComponentExternalKind::Instance => ComponentIndexSpace::Instance,
            ComponentExternalKind::Component => ComponentIndexSpace::Component,
        }
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Export
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasTarget {
    pub count: u32,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alias {
    InstanceExport {
        kind: ComponentExternalKind,
        instance_idx: InstanceIdx,
        name: String,
    },
    CoreInstanceExport {
        kind: ExportKind,
        instance_idx: InstanceIdx,
        name: String,
    },
    Outer {
        kind: OuterAliasKind,
        target: AliasTarget,
    },
}

impl Section<ComponentIndexSpace, ComponentSectionType> for Alias {
    fn index_space(&self) -> ComponentIndexSpace {
        match self {
            Alias::InstanceExport {
                kind: ComponentExternalKind::Component,
                ..
            } => ComponentIndexSpace::Component,
            Alias::InstanceExport {
                kind: ComponentExternalKind::Func,
                ..
            } => ComponentIndexSpace::Func,
            Alias::InstanceExport {
                kind: ComponentExternalKind::Instance,
                ..
            } => ComponentIndexSpace::Instance,
            Alias::InstanceExport {
                kind: ComponentExternalKind::Module,
                ..
            } => ComponentIndexSpace::Module,
            Alias::InstanceExport {
                kind: ComponentExternalKind::Type,
                ..
            } => ComponentIndexSpace::Type,
            Alias::InstanceExport {
                kind: ComponentExternalKind::Value,
                ..
            } => ComponentIndexSpace::Value,
            Alias::CoreInstanceExport {
                kind: ExportKind::Func,
                ..
            } => ComponentIndexSpace::CoreFunc,
            Alias::CoreInstanceExport {
                kind: ExportKind::Global,
                ..
            } => ComponentIndexSpace::CoreGlobal,
            Alias::CoreInstanceExport {
                kind: ExportKind::Mem,
                ..
            } => ComponentIndexSpace::CoreMem,
            Alias::CoreInstanceExport {
                kind: ExportKind::Table,
                ..
            } => ComponentIndexSpace::CoreTable,
            Alias::Outer {
                kind: OuterAliasKind::Component,
                ..
            } => ComponentIndexSpace::Component,
            Alias::Outer {
                kind: OuterAliasKind::CoreType,
                ..
            } => ComponentIndexSpace::CoreType,
            Alias::Outer {
                kind: OuterAliasKind::Type,
                ..
            } => ComponentIndexSpace::Type,
            Alias::Outer {
                kind: OuterAliasKind::CoreModule,
                ..
            } => ComponentIndexSpace::Module,
        }
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Alias
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalOption {
    Utf8,
    Utf16,
    CompactUtf16,
    Memory(MemIdx),
    Realloc(FuncIdx),
    PostReturn(FuncIdx),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Canon {
    Lift {
        func_idx: FuncIdx,
        opts: Vec<CanonicalOption>,
        function_type: ComponentTypeIdx,
    },
    Lower {
        func_idx: ComponentFuncIdx,
        opts: Vec<CanonicalOption>,
    },
    ResourceNew {
        type_idx: ComponentTypeIdx,
    },
    ResourceDrop {
        type_idx: ComponentTypeIdx,
    },
    ResourceRep {
        type_idx: ComponentTypeIdx,
    },
}

impl Section<ComponentIndexSpace, ComponentSectionType> for Canon {
    fn index_space(&self) -> ComponentIndexSpace {
        match self {
            Canon::Lift { .. } => ComponentIndexSpace::Func,
            Canon::Lower { .. } => ComponentIndexSpace::CoreFunc,
            Canon::ResourceNew { .. } => ComponentIndexSpace::Func,
            Canon::ResourceDrop { .. } => ComponentIndexSpace::Func,
            Canon::ResourceRep { .. } => ComponentIndexSpace::Func,
        }
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Canon
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentStart {
    func_idx: ComponentFuncIdx,
    args: Vec<ValueIdx>,
    results: u32,
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentStart {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Start
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Start
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentImport {
    name: ComponentExternName,
    desc: ExternDesc,
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentImport {
    fn index_space(&self) -> ComponentIndexSpace {
        match self.desc {
            ExternDesc::Module(_) => ComponentIndexSpace::Module,
            ExternDesc::Func(_) => ComponentIndexSpace::Func,
            ExternDesc::Val(_) => ComponentIndexSpace::Value,
            ExternDesc::Type(_) => ComponentIndexSpace::Type,
            ExternDesc::Instance(_) => ComponentIndexSpace::Instance,
            ExternDesc::Component(_) => ComponentIndexSpace::Component,
        }
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Import
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleDeclaration {
    Type {
        typ: FuncType,
    },
    Export {
        name: String,
        desc: TypeRef,
    },
    OuterAlias {
        kind: OuterAliasKind,
        target: AliasTarget,
    },
    Import {
        import: Import,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreType {
    Function(FuncType),
    Module(Vec<ModuleDeclaration>),
}

impl Section<ComponentIndexSpace, ComponentSectionType> for CoreType {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::CoreType
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::CoreType
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantCase {
    pub name: String,
    pub typ: Option<ComponentValType>,
    refines: Option<u32>, // TODO
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentDefinedType {
    Primitive {
        typ: PrimitiveValueType,
    },
    Record {
        fields: Vec<(String, ComponentValType)>,
    },
    Variant {
        cases: Vec<VariantCase>,
    },
    List {
        elem: ComponentValType,
    },
    Tuple {
        elems: Vec<ComponentValType>,
    },
    Flags {
        names: Vec<String>,
    },
    Enum {
        names: Vec<String>,
    },
    Union {
        types: Vec<ComponentValType>,
    },
    Option {
        typ: ComponentValType,
    },
    Result {
        ok: Option<ComponentValType>,
        err: Option<ComponentValType>,
    },
    Owned {
        type_idx: ComponentTypeIdx,
    },
    Borrowed {
        type_idx: ComponentTypeIdx,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentFuncType {
    pub params: Vec<(String, ComponentValType)>,
    pub result: ComponentFuncResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentFuncResult {
    Unnamed(ComponentValType),
    Named(Vec<(String, ComponentValType)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentTypeDeclaration {
    Core(CoreType),
    Type(ComponentType),
    Alias(Alias),
    Import(ComponentImport),
    Export {
        name: ComponentExternName,
        desc: ExternDesc,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceTypeDeclaration {
    Core(CoreType),
    Type(ComponentType),
    Alias(Alias),
    Export {
        name: ComponentExternName,
        desc: ExternDesc,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentType {
    Defined(ComponentDefinedType),
    Func(ComponentFuncType),
    Component(Vec<ComponentTypeDeclaration>),
    Instance(Vec<InstanceTypeDeclaration>),
    Resource {
        representation: ValType,
        destructor: Option<ComponentFuncIdx>,
    },
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentType {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Type
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Type
    }
}

impl Section<ComponentIndexSpace, ComponentSectionType> for Custom {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Custom
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Custom
    }
}

struct ComponentInner<Expr: Debug + Clone + PartialEq + 'static> {
    sections: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Expr>>,
}

impl<Expr: Debug + Clone + PartialEq> ComponentInner<Expr> {
    fn new(
        sections: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Expr>>,
    ) -> Self {
        Self { sections }
    }
}

impl<Expr: Debug + Clone + PartialEq> Debug for ComponentInner<Expr> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.sections.fmt(f)
    }
}

impl<Expr: Debug + Clone + PartialEq> PartialEq for ComponentInner<Expr> {
    fn eq(&self, other: &Self) -> bool {
        self.sections.eq(&other.sections)
    }
}

#[derive(Debug, Clone)]
pub struct Component<Expr: Debug + Clone + PartialEq + 'static> {
    inner: Arc<Mutex<ComponentInner<Expr>>>,
}

impl<Expr: Debug + Clone + PartialEq>
    From<Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Expr>>>
    for Component<Expr>
{
    fn from(
        value: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Expr>>,
    ) -> Self {
        Component {
            inner: Arc::new(Mutex::new(ComponentInner::new(value))),
        }
    }
}

impl<Expr: Debug + Clone + PartialEq> PartialEq for Component<Expr> {
    fn eq(&self, other: &Self) -> bool {
        let inner = self.inner.lock().unwrap();
        let other_inner = other.inner.lock().unwrap();
        inner.eq(&other_inner)
    }
}

impl<Expr: Debug + Clone + PartialEq> Section<ComponentIndexSpace, ComponentSectionType>
    for Component<Expr>
{
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Component
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Component
    }
}
