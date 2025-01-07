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

use crate::component::parser::parse_component;
use crate::core::{
    Custom, Data, Export, FuncIdx, FuncType, Import, MemIdx, Module, RetainsCustomSection,
    RetainsInstructions, TryFromExprSource, TypeRef, ValType,
};
use crate::{
    metadata, new_component_section_cache, AstCustomization, IndexSpace, Section, SectionCache,
    SectionIndex, SectionType, Sections,
};
use mappable_rc::Mrc;
use std::fmt::{Debug, Formatter};

#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "writer")]
pub mod writer;

/// The Component Model section nodes.
///
/// See [Section] for more information.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentSection<Ast: AstCustomization + 'static> {
    Module(Module<Ast>),
    CoreInstance(Instance),
    CoreType(CoreType),
    Component(Component<Ast>),
    Instance(ComponentInstance),
    Alias(Alias),
    Type(ComponentType),
    Canon(Canon),
    Start(ComponentStart),
    Import(ComponentImport),
    Export(ComponentExport),
    Custom(Ast::Custom),
}

#[allow(unused)]
impl<Ast: AstCustomization> ComponentSection<Ast> {
    pub fn as_module(&self) -> &Module<Ast> {
        match self {
            ComponentSection::Module(module) => module,
            _ => panic!("Expected module section, got {}", self.type_name()),
        }
    }

    pub fn as_core_instance(&self) -> &Instance {
        match self {
            ComponentSection::CoreInstance(instance) => instance,
            _ => panic!("Expected core instance section, got {}", self.type_name()),
        }
    }

    pub fn as_core_type(&self) -> &CoreType {
        match self {
            ComponentSection::CoreType(core_type) => core_type,
            _ => panic!("Expected core type section, got {}", self.type_name()),
        }
    }

    pub fn as_component(&self) -> &Component<Ast> {
        match self {
            ComponentSection::Component(component) => component,
            _ => panic!("Expected component section, got {}", self.type_name()),
        }
    }

    pub fn as_instance(&self) -> &ComponentInstance {
        match self {
            ComponentSection::Instance(component_instance) => component_instance,
            _ => panic!(
                "Expected component instance section, got {}",
                self.type_name()
            ),
        }
    }

    pub fn as_alias(&self) -> &Alias {
        match self {
            ComponentSection::Alias(alias) => alias,
            _ => panic!("Expected alias section, got {}", self.type_name()),
        }
    }

    pub fn as_type(&self) -> &ComponentType {
        match self {
            ComponentSection::Type(component_type) => component_type,
            _ => panic!("Expected type section, got {}", self.type_name()),
        }
    }

    pub fn as_canon(&self) -> &Canon {
        match self {
            ComponentSection::Canon(canon) => canon,
            _ => panic!("Expected canon section, got {}", self.type_name()),
        }
    }

    pub fn as_start(&self) -> &ComponentStart {
        match self {
            ComponentSection::Start(start) => start,
            _ => panic!("Expected start section, got {}", self.type_name()),
        }
    }

    pub fn as_import(&self) -> &ComponentImport {
        match self {
            ComponentSection::Import(import) => import,
            _ => panic!("Expected import section, got {}", self.type_name()),
        }
    }

    pub fn as_export(&self) -> &ComponentExport {
        match self {
            ComponentSection::Export(export) => export,
            _ => panic!("Expected export section, got {}", self.type_name()),
        }
    }

    pub fn as_custom(&self) -> &Ast::Custom {
        match self {
            ComponentSection::Custom(custom) => custom,
            _ => panic!("Expected custom section, got {}", self.type_name()),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            ComponentSection::Module(_) => "module",
            ComponentSection::CoreInstance(_) => "core instance",
            ComponentSection::CoreType(_) => "core type",
            ComponentSection::Component(_) => "component",
            ComponentSection::Instance(_) => "instance",
            ComponentSection::Alias(_) => "alias",
            ComponentSection::Type(_) => "type",
            ComponentSection::Canon(_) => "canonical function",
            ComponentSection::Start(_) => "start",
            ComponentSection::Import(_) => "import",
            ComponentSection::Export(_) => "export",
            ComponentSection::Custom(_) => "custom",
        }
    }
}

impl<Ast: AstCustomization> Section<ComponentIndexSpace, ComponentSectionType>
    for ComponentSection<Ast>
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

/// The Component Model section types.
///
/// See [SectionType] for more information.
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

/// The Component Model index spaces.
///
/// See [IndexSpace] for more information.
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

pub type ComponentFuncIdx = u32;
pub type ComponentTypeIdx = u32;
pub type ModuleIdx = u32;
pub type ComponentIdx = u32;

#[allow(unused)]
pub type CoreInstanceIdx = u32;
pub type InstanceIdx = u32;
pub type ValueIdx = u32;

#[allow(unused)]
pub type StartIdx = u32;

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
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
#[cfg_attr(feature = "poem_openapi", derive(poem_openapi::Enum))]
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

impl ComponentExternName {
    pub fn as_string(&self) -> String {
        let str: &str = self.into();
        str.to_string()
    }
}

impl<'a> From<&'a ComponentExternName> for &'a str {
    fn from(value: &'a ComponentExternName) -> Self {
        match value {
            ComponentExternName::Name(name) => name,
        }
    }
}

impl PartialEq<String> for ComponentExternName {
    fn eq(&self, other: &String) -> bool {
        match self {
            ComponentExternName::Name(name) => name == other,
        }
    }
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
pub enum ComponentTypeRef {
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
    pub desc: Option<ComponentTypeRef>,
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
            Canon::ResourceNew { .. } => ComponentIndexSpace::CoreFunc,
            Canon::ResourceDrop { .. } => ComponentIndexSpace::CoreFunc,
            Canon::ResourceRep { .. } => ComponentIndexSpace::CoreFunc,
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
    pub name: ComponentExternName,
    pub desc: ComponentTypeRef,
}

impl Section<ComponentIndexSpace, ComponentSectionType> for ComponentImport {
    fn index_space(&self) -> ComponentIndexSpace {
        match self.desc {
            ComponentTypeRef::Module(_) => ComponentIndexSpace::Module,
            ComponentTypeRef::Func(_) => ComponentIndexSpace::Func,
            ComponentTypeRef::Val(_) => ComponentIndexSpace::Value,
            ComponentTypeRef::Type(_) => ComponentIndexSpace::Type,
            ComponentTypeRef::Instance(_) => ComponentIndexSpace::Instance,
            ComponentTypeRef::Component(_) => ComponentIndexSpace::Component,
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
    /// The index of the variant case that is refined by this one.
    pub refines: Option<u32>,
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
        desc: ComponentTypeRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentTypeDeclarations(Vec<ComponentTypeDeclaration>);

impl ComponentTypeDeclarations {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceTypeDeclaration {
    Core(CoreType),
    Type(ComponentType),
    Alias(Alias),
    Export {
        name: ComponentExternName,
        desc: ComponentTypeRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceTypeDeclarations(Vec<InstanceTypeDeclaration>);

impl InstanceTypeDeclarations {
    pub fn find_export(&self, name: &str) -> Option<&ComponentTypeRef> {
        self.0.iter().find_map(|decl| match decl {
            InstanceTypeDeclaration::Export {
                name: ComponentExternName::Name(n),
                desc,
            } if n == name => Some(desc),
            _ => None,
        })
    }

    pub fn get_component_type(
        &self,
        component_type_idx: ComponentTypeIdx,
    ) -> Option<&InstanceTypeDeclaration> {
        let mut idx = 0;
        let mut result = None;
        for decl in &self.0 {
            match decl {
                InstanceTypeDeclaration::Type(_)
                | InstanceTypeDeclaration::Alias(Alias::Outer {
                    kind: OuterAliasKind::Type,
                    ..
                })
                | InstanceTypeDeclaration::Alias(Alias::InstanceExport {
                    kind: ComponentExternalKind::Type,
                    ..
                })
                | InstanceTypeDeclaration::Export {
                    desc: ComponentTypeRef::Type(_),
                    ..
                } => {
                    if component_type_idx == idx {
                        result = Some(decl);
                        break;
                    } else {
                        idx += 1;
                    }
                }
                _ => {}
            }
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentType {
    Defined(ComponentDefinedType),
    Func(ComponentFuncType),
    Component(ComponentTypeDeclarations),
    Instance(InstanceTypeDeclarations),
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

type ComponentSectionCache<T, Ast> =
    SectionCache<T, ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>;

type ComponentSectionIndex<Ast> =
    SectionIndex<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>;

/// The top level node of the Component Model AST
pub struct Component<Ast: AstCustomization + 'static> {
    sections: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>,

    imports: ComponentSectionCache<ComponentImport, Ast>,
    exports: ComponentSectionCache<ComponentExport, Ast>,
    core_instances: ComponentSectionCache<Instance, Ast>,
    instances: ComponentSectionCache<ComponentInstance, Ast>,
    component_types: ComponentSectionCache<ComponentType, Ast>,
    core_types: ComponentSectionCache<CoreType, Ast>,
    canons: ComponentSectionCache<Canon, Ast>,
    aliases: ComponentSectionCache<Alias, Ast>,
    components: ComponentSectionCache<Component<Ast>, Ast>,
    modules: ComponentSectionCache<Module<Ast>, Ast>,
    customs: ComponentSectionCache<Ast::Custom, Ast>,

    core_instance_index: ComponentSectionIndex<Ast>,
    instance_index: ComponentSectionIndex<Ast>,
    component_type_index: ComponentSectionIndex<Ast>,
    core_func_index: ComponentSectionIndex<Ast>,
    component_index: ComponentSectionIndex<Ast>,
    component_func_index: ComponentSectionIndex<Ast>,
    value_index: ComponentSectionIndex<Ast>,
    module_index: ComponentSectionIndex<Ast>,
}

#[cfg(feature = "parser")]
impl<Ast> Component<Ast>
where
    Ast: AstCustomization,
    Ast::Expr: TryFromExprSource,
    Ast::Data: From<Data<Ast::Expr>>,
    Ast::Custom: From<Custom>,
{
    /// Parses a Component Model AST from the binary WASM byte array
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let parser = wasmparser::Parser::new(0);
        let (component, _) = parse_component(parser, bytes)?;
        Ok(component)
    }
}

#[cfg(feature = "writer")]
impl<Ast> Component<Ast>
where
    Ast: AstCustomization,
    Ast::Expr: RetainsInstructions,
    Ast::Data: Into<Data<Ast::Expr>>,
    Ast::Custom: Into<Custom>,
{
    /// Serializes a WASM Component into a binary WASM byte array
    pub fn into_bytes(self) -> Result<Vec<u8>, String> {
        let encoder: wasm_encoder::Component = self.try_into()?;
        Ok(encoder.finish())
    }
}

impl<Ast: AstCustomization> Component<Ast> {
    /// Creates an empty component
    pub fn empty() -> Self {
        Self::new(Sections::new())
    }

    pub(crate) fn new(
        sections: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>,
    ) -> Self {
        Self {
            sections,

            imports: new_component_section_cache!(Import),
            exports: new_component_section_cache!(Export),
            core_instances: new_component_section_cache!(CoreInstance),
            instances: new_component_section_cache!(Instance),
            component_types: new_component_section_cache!(Type),
            core_types: new_component_section_cache!(CoreType),
            canons: new_component_section_cache!(Canon),
            aliases: new_component_section_cache!(Alias),
            components: new_component_section_cache!(Component),
            modules: new_component_section_cache!(Module),
            customs: new_component_section_cache!(Custom),
            core_instance_index: SectionIndex::new(ComponentIndexSpace::CoreInstance),
            instance_index: SectionIndex::new(ComponentIndexSpace::Instance),
            component_type_index: SectionIndex::new(ComponentIndexSpace::Type),
            core_func_index: SectionIndex::new(ComponentIndexSpace::CoreFunc),
            component_index: SectionIndex::new(ComponentIndexSpace::Component),
            component_func_index: SectionIndex::new(ComponentIndexSpace::Func),
            value_index: SectionIndex::new(ComponentIndexSpace::Value),
            module_index: SectionIndex::new(ComponentIndexSpace::Module),
        }
    }

    /// Gets all the imports defined in this component
    pub fn imports(&self) -> Vec<Mrc<ComponentImport>> {
        self.imports.populate(&self.sections);
        self.imports.all()
    }

    /// Gets all the exports defined in this component
    pub fn exports(&self) -> Vec<Mrc<ComponentExport>> {
        self.exports.populate(&self.sections);
        self.exports.all()
    }

    /// Gets all the core instances defined in this component
    pub fn core_instances(&self) -> Vec<Mrc<Instance>> {
        self.core_instances.populate(&self.sections);
        self.core_instances.all()
    }

    /// Gets all the component instances defined in this component
    pub fn instances(&self) -> Vec<Mrc<ComponentInstance>> {
        self.instances.populate(&self.sections);
        self.instances.all()
    }

    /// Gets all the component types defined in this component
    pub fn component_types(&self) -> Vec<Mrc<ComponentType>> {
        self.component_types.populate(&self.sections);
        self.component_types.all()
    }

    /// Gets all the core types defined in this component
    pub fn core_types(&self) -> Vec<Mrc<CoreType>> {
        self.core_types.populate(&self.sections);
        self.core_types.all()
    }

    /// Gets all the canonical function definitions of this component
    pub fn canons(&self) -> Vec<Mrc<Canon>> {
        self.canons.populate(&self.sections);
        self.canons.all()
    }

    /// Gets all the aliases defined in this component
    pub fn aliases(&self) -> Vec<Mrc<Alias>> {
        self.aliases.populate(&self.sections);
        self.aliases.all()
    }

    /// Gets all the inner components defined in this component
    pub fn components(&self) -> Vec<Mrc<Component<Ast>>> {
        self.components.populate(&self.sections);
        self.components.all()
    }

    /// Gets all the inner core modules defined in this component
    pub fn modules(&self) -> Vec<Mrc<Module<Ast>>> {
        self.modules.populate(&self.sections);
        self.modules.all()
    }

    /// Gets all the custom sections defined in this component
    pub fn customs(&self) -> Vec<Mrc<Ast::Custom>> {
        self.customs.populate(&self.sections);
        self.customs.all()
    }

    /// Returns the core instance referenced by the given index.
    pub fn get_core_instance(&self, core_instance_idx: CoreInstanceIdx) -> Option<Mrc<Instance>> {
        self.core_instance_index.populate(&self.sections);
        match self.core_instance_index.get(&core_instance_idx) {
            Some(section) => match &*section {
                ComponentSection::CoreInstance(_) => {
                    Some(Mrc::map(section, |section| section.as_core_instance()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns the component instance referenced by the given index.
    pub fn get_instance_wrapped(
        &self,
        instance_idx: InstanceIdx,
    ) -> Option<Mrc<ComponentSection<Ast>>> {
        self.instance_index.populate(&self.sections);
        self.instance_index.get(&instance_idx)
    }

    /// Returns the component instance referenced by the given index.
    pub fn get_instance(&self, instance_idx: InstanceIdx) -> Option<Mrc<ComponentInstance>> {
        match self.get_instance_wrapped(instance_idx) {
            Some(section) => match &*section {
                ComponentSection::Instance(_) => {
                    Some(Mrc::map(section, |section| section.as_instance()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns the component type referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - ComponentType
    /// - Alias
    /// - ComponentExport
    /// - ComponentImport
    pub fn get_component_type(
        &self,
        component_type_idx: ComponentTypeIdx,
    ) -> Option<Mrc<ComponentSection<Ast>>> {
        self.component_type_index.populate(&self.sections);
        self.component_type_index.get(&component_type_idx)
    }

    /// Returns the core function referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - Canon
    /// - Alias
    pub fn get_core_func(&self, core_func_idx: FuncIdx) -> Option<Mrc<ComponentSection<Ast>>> {
        self.core_func_index.populate(&self.sections);
        self.core_func_index.get(&core_func_idx)
    }

    /// Returns the component referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - Component
    /// - Alias
    /// - ComponentExport
    /// - ComponentImport
    pub fn get_component(&self, component_idx: ComponentIdx) -> Option<Mrc<ComponentSection<Ast>>> {
        self.component_index.populate(&self.sections);
        self.component_index.get(&component_idx)
    }

    /// Returns the component function referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - Canon
    /// - Alias
    /// - ComponentExport
    /// - ComponentImport
    pub fn get_component_func(
        &self,
        component_func_idx: ComponentFuncIdx,
    ) -> Option<Mrc<ComponentSection<Ast>>> {
        self.component_func_index.populate(&self.sections);
        self.component_func_index.get(&component_func_idx)
    }

    /// Returns the value referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - Alias
    /// - ComponentExport
    /// - ComponentImport
    pub fn get_value(&self, value_idx: ValueIdx) -> Option<Mrc<ComponentSection<Ast>>> {
        self.value_index.populate(&self.sections);
        self.value_index.get(&value_idx)
    }

    /// Returns the module referenced by the given index.
    ///
    /// It can be one of the following section types:
    /// - Module
    /// - Alias
    /// - ComponentExport
    /// - ComponentImport
    pub fn get_module(&self, module_idx: ModuleIdx) -> Option<Mrc<ComponentSection<Ast>>> {
        self.module_index.populate(&self.sections);
        self.module_index.get(&module_idx)
    }

    /// Converts this component into a sequence of component sections.
    pub fn into_sections(mut self) -> Vec<Mrc<ComponentSection<Ast>>> {
        self.sections.take_all()
    }

    /// Converts this component into a sequence of grouped component sections, exactly as it would be in the binary WASM format.
    pub fn into_grouped(self) -> Vec<(ComponentSectionType, Vec<Mrc<ComponentSection<Ast>>>)> {
        self.sections.into_grouped()
    }
}

impl<Ast> Component<Ast>
where
    Ast: AstCustomization,
    Ast::Custom: RetainsCustomSection,
{
    #[cfg(feature = "metadata")]
    pub fn get_metadata(&self) -> Option<metadata::Metadata> {
        let mut producers = None;
        let mut registry_metadata = None;
        let mut name = None;

        for custom in self.customs() {
            if custom.name() == "producers" {
                producers = wasm_metadata::Producers::from_bytes(custom.data(), 0).ok();
            } else if custom.name() == "registry-metadata" {
                registry_metadata =
                    wasm_metadata::RegistryMetadata::from_bytes(custom.data(), 0).ok();
            } else if custom.name() == "name" {
                name = wasm_metadata::ModuleNames::from_bytes(custom.data(), 0)
                    .ok()
                    .and_then(|n| n.get_name().cloned());
            } else if custom.name() == "component-name" {
                name = wasm_metadata::ModuleNames::from_bytes(custom.data(), 0)
                    .ok()
                    .and_then(|n| n.get_name().cloned());
            }
        }

        if producers.is_some() || registry_metadata.is_some() || name.is_some() {
            Some(metadata::Metadata {
                name,
                producers: producers.map(|p| p.into()),
                registry_metadata,
            })
        } else {
            None
        }
    }

    /// Collects all the producers sections from this component and its subsections
    #[cfg(feature = "metadata")]
    pub fn get_all_producers(&self) -> Vec<metadata::Producers> {
        let mut result = Vec::new();
        if let Some(producers) = self.get_metadata().and_then(|m| m.producers) {
            result.push(producers);
        }
        for module in self.modules() {
            if let Some(producers) = module.get_metadata().and_then(|m| m.producers) {
                result.push(producers);
            }
        }
        for component in self.components() {
            result.append(&mut component.get_all_producers());
        }
        result
    }
}

impl<Ast: AstCustomization>
    From<Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>>
    for Component<Ast>
{
    fn from(
        value: Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>,
    ) -> Self {
        Component::new(value)
    }
}

impl<Ast: AstCustomization> PartialEq for Component<Ast> {
    fn eq(&self, other: &Self) -> bool {
        self.sections.eq(&other.sections)
    }
}

impl<Ast: AstCustomization> Debug for Component<Ast> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.sections.fmt(f)
    }
}

impl<Ast: AstCustomization> Clone for Component<Ast> {
    fn clone(&self) -> Self {
        Component::new(self.sections.clone())
    }
}

impl<Ast: AstCustomization> Section<ComponentIndexSpace, ComponentSectionType> for Component<Ast> {
    fn index_space(&self) -> ComponentIndexSpace {
        ComponentIndexSpace::Component
    }

    fn section_type(&self) -> ComponentSectionType {
        ComponentSectionType::Component
    }
}
