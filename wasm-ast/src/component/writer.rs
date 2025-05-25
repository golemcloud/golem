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

use crate::component::*;
use crate::core::{Data, ExportDesc, RetainsInstructions};

impl From<&InstantiationArgRef> for wasm_encoder::ModuleArg {
    fn from(value: &InstantiationArgRef) -> Self {
        match value {
            InstantiationArgRef::Instance(instance_idx) => {
                wasm_encoder::ModuleArg::Instance(*instance_idx)
            }
        }
    }
}

impl From<&InstantiationArg> for (String, wasm_encoder::ModuleArg) {
    fn from(value: &InstantiationArg) -> Self {
        (value.name.clone(), (&value.arg_ref).into())
    }
}

impl From<&Export> for (String, wasm_encoder::ExportKind, u32) {
    fn from(value: &Export) -> Self {
        let (kind, index) = match value.desc {
            ExportDesc::Func(func_idx) => (wasm_encoder::ExportKind::Func, func_idx),
            ExportDesc::Table(table_idx) => (wasm_encoder::ExportKind::Table, table_idx),
            ExportDesc::Mem(mem_idx) => (wasm_encoder::ExportKind::Memory, mem_idx),
            ExportDesc::Global(global_idx) => (wasm_encoder::ExportKind::Global, global_idx),
        };
        (value.name.clone(), kind, index)
    }
}

impl From<&Instance> for wasm_encoder::InstanceSection {
    fn from(value: &Instance) -> Self {
        let mut section = wasm_encoder::InstanceSection::new();
        add_to_core_instance_section(&mut section, value);
        section
    }
}

fn add_to_core_instance_section(section: &mut wasm_encoder::InstanceSection, value: &Instance) {
    match value {
        Instance::Instantiate { module_idx, args } => {
            section.instantiate(
                *module_idx,
                args.iter()
                    .map(|instantiation_arg| instantiation_arg.into()),
            );
        }
        Instance::FromExports { exports } => {
            section.export_items(exports.iter().map(|export| export.into()));
        }
    }
}

fn add_to_module_type(
    module_type: &mut wasm_encoder::ModuleType,
    value: &Vec<ModuleDeclaration>,
) -> Result<(), String> {
    for module_declaration in value {
        match module_declaration {
            ModuleDeclaration::Type { typ } => {
                module_type.ty().function(
                    typ.input.values.iter().map(|val_type| val_type.into()),
                    typ.output.values.iter().map(|val_type| val_type.into()),
                );
            }
            ModuleDeclaration::Export { name, desc } => {
                module_type.export(name, desc.into());
            }
            ModuleDeclaration::OuterAlias { kind, target } => match kind {
                OuterAliasKind::CoreType => {
                    module_type.alias_outer_core_type(target.count, target.index);
                }
                OuterAliasKind::CoreModule => {
                    return Err("CoreModule outer alias is not supported".to_string())
                }
                OuterAliasKind::Type => return Err("Type outer alias is not supported".to_string()),
                OuterAliasKind::Component => {
                    return Err("Component outer alias is not supported".to_string())
                }
            },
            ModuleDeclaration::Import { import } => {
                module_type.import(&import.module, &import.name, (&import.desc).into());
            }
        }
    }
    Ok(())
}

impl TryFrom<&CoreType> for wasm_encoder::CoreTypeSection {
    type Error = String;

    fn try_from(value: &CoreType) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::CoreTypeSection::new();
        add_to_core_type_section(&mut section, value)?;
        Ok(section)
    }
}

fn add_to_core_type_encoder(
    encoder: wasm_encoder::ComponentCoreTypeEncoder,
    value: &CoreType,
) -> Result<(), String> {
    match value {
        CoreType::Function(func_type) => {
            encoder.core().function(
                func_type
                    .input
                    .values
                    .iter()
                    .map(|val_type| val_type.into()),
                func_type
                    .output
                    .values
                    .iter()
                    .map(|val_type| val_type.into()),
            );
        }
        CoreType::Module(module_declarations) => {
            let mut module_type = wasm_encoder::ModuleType::new();
            add_to_module_type(&mut module_type, module_declarations)?;
            encoder.module(&module_type);
        }
    }
    Ok(())
}

fn add_to_core_type_section(
    section: &mut wasm_encoder::CoreTypeSection,
    value: &CoreType,
) -> Result<(), String> {
    add_to_core_type_encoder(section.ty(), value)
}

impl From<&ComponentExternalKind> for wasm_encoder::ComponentExportKind {
    fn from(value: &ComponentExternalKind) -> Self {
        match value {
            ComponentExternalKind::Module => wasm_encoder::ComponentExportKind::Module,
            ComponentExternalKind::Func => wasm_encoder::ComponentExportKind::Func,
            ComponentExternalKind::Value => wasm_encoder::ComponentExportKind::Value,
            ComponentExternalKind::Type => wasm_encoder::ComponentExportKind::Type,
            ComponentExternalKind::Instance => wasm_encoder::ComponentExportKind::Instance,
            ComponentExternalKind::Component => wasm_encoder::ComponentExportKind::Component,
        }
    }
}

impl From<&ComponentInstantiationArg> for (String, wasm_encoder::ComponentExportKind, u32) {
    fn from(value: &ComponentInstantiationArg) -> Self {
        (value.name.clone(), (&value.kind).into(), value.idx)
    }
}

impl<'a> From<&'a ComponentExport> for (&'a str, wasm_encoder::ComponentExportKind, u32) {
    fn from(value: &'a ComponentExport) -> Self {
        let name: &'a str = (&value.name).into();
        (name, (&value.kind).into(), value.idx)
    }
}

impl From<&ComponentInstance> for wasm_encoder::ComponentInstanceSection {
    fn from(value: &ComponentInstance) -> Self {
        let mut section = wasm_encoder::ComponentInstanceSection::new();
        add_to_component_instance_section(&mut section, value);
        section
    }
}

fn add_to_component_instance_section(
    section: &mut wasm_encoder::ComponentInstanceSection,
    value: &ComponentInstance,
) {
    match value {
        ComponentInstance::Instantiate {
            component_idx,
            args,
        } => {
            section.instantiate(
                *component_idx,
                args.iter()
                    .map(|instantiation_arg| instantiation_arg.into()),
            );
        }
        ComponentInstance::FromExports { exports } => {
            section.export_items(exports.iter().map(|export| export.into()));
        }
    }
}

impl From<&OuterAliasKind> for wasm_encoder::ComponentOuterAliasKind {
    fn from(value: &OuterAliasKind) -> Self {
        match value {
            OuterAliasKind::CoreModule => wasm_encoder::ComponentOuterAliasKind::CoreModule,
            OuterAliasKind::CoreType => wasm_encoder::ComponentOuterAliasKind::CoreType,
            OuterAliasKind::Type => wasm_encoder::ComponentOuterAliasKind::Type,
            OuterAliasKind::Component => wasm_encoder::ComponentOuterAliasKind::Component,
        }
    }
}

impl From<&ExportKind> for wasm_encoder::ExportKind {
    fn from(value: &ExportKind) -> Self {
        match value {
            ExportKind::Func => wasm_encoder::ExportKind::Func,
            ExportKind::Table => wasm_encoder::ExportKind::Table,
            ExportKind::Mem => wasm_encoder::ExportKind::Memory,
            ExportKind::Global => wasm_encoder::ExportKind::Global,
        }
    }
}

impl<'a> From<&'a Alias> for wasm_encoder::Alias<'a> {
    fn from(value: &'a Alias) -> Self {
        match value {
            Alias::InstanceExport {
                kind,
                instance_idx,
                name,
            } => wasm_encoder::Alias::InstanceExport {
                instance: *instance_idx,
                kind: kind.into(),
                name: name.as_str(),
            },
            Alias::CoreInstanceExport {
                kind,
                instance_idx,
                name,
            } => wasm_encoder::Alias::CoreInstanceExport {
                instance: *instance_idx,
                kind: kind.into(),
                name: name.as_str(),
            },
            Alias::Outer { kind, target } => wasm_encoder::Alias::Outer {
                count: target.count,
                kind: kind.into(),
                index: target.index,
            },
        }
    }
}

impl From<&Alias> for wasm_encoder::ComponentAliasSection {
    fn from(value: &Alias) -> Self {
        let mut section = wasm_encoder::ComponentAliasSection::new();
        add_to_alias_section(&mut section, value);
        section
    }
}

fn add_to_alias_section(section: &mut wasm_encoder::ComponentAliasSection, value: &Alias) {
    section.alias(value.into());
}

impl TryFrom<&ComponentType> for wasm_encoder::ComponentTypeSection {
    type Error = String;

    fn try_from(value: &ComponentType) -> Result<Self, Self::Error> {
        let mut section = wasm_encoder::ComponentTypeSection::new();
        add_to_type_section(&mut section, value)?;
        Ok(section)
    }
}

impl From<&TypeBounds> for wasm_encoder::TypeBounds {
    fn from(value: &TypeBounds) -> Self {
        match value {
            TypeBounds::Eq(component_type_idx) => wasm_encoder::TypeBounds::Eq(*component_type_idx),
            TypeBounds::SubResource => wasm_encoder::TypeBounds::SubResource,
        }
    }
}

impl From<&ComponentTypeRef> for wasm_encoder::ComponentTypeRef {
    fn from(value: &ComponentTypeRef) -> Self {
        match value {
            ComponentTypeRef::Module(component_type_idx) => {
                wasm_encoder::ComponentTypeRef::Module(*component_type_idx)
            }
            ComponentTypeRef::Func(component_type_idx) => {
                wasm_encoder::ComponentTypeRef::Func(*component_type_idx)
            }
            ComponentTypeRef::Val(component_val_type) => {
                wasm_encoder::ComponentTypeRef::Value(component_val_type.into())
            }
            ComponentTypeRef::Type(type_bounds) => {
                wasm_encoder::ComponentTypeRef::Type(type_bounds.into())
            }
            ComponentTypeRef::Instance(component_type_idx) => {
                wasm_encoder::ComponentTypeRef::Instance(*component_type_idx)
            }
            ComponentTypeRef::Component(component_type_idx) => {
                wasm_encoder::ComponentTypeRef::Component(*component_type_idx)
            }
        }
    }
}

fn add_declaration_to_component_type(
    component_type: &mut wasm_encoder::ComponentType,
    value: &ComponentTypeDeclaration,
) -> Result<(), String> {
    match value {
        ComponentTypeDeclaration::Core(core_type) => {
            add_to_core_type_encoder(component_type.core_type(), core_type)?;
        }
        ComponentTypeDeclaration::Type(ct) => {
            add_to_component_type(component_type.ty(), ct)?;
        }
        ComponentTypeDeclaration::Alias(alias) => {
            component_type.alias(alias.into());
        }
        ComponentTypeDeclaration::Import(import) => {
            component_type.import((&import.name).into(), (&import.desc).into());
        }
        ComponentTypeDeclaration::Export { name, desc } => {
            component_type.export(name.into(), desc.into());
        }
    }
    Ok(())
}

fn add_declaration_to_instance_type(
    instance_type: &mut wasm_encoder::InstanceType,
    value: &InstanceTypeDeclaration,
) -> Result<(), String> {
    match value {
        InstanceTypeDeclaration::Core(core_type) => {
            add_to_core_type_encoder(instance_type.core_type(), core_type)?;
        }
        InstanceTypeDeclaration::Type(ct) => {
            add_to_component_type(instance_type.ty(), ct)?;
        }
        InstanceTypeDeclaration::Alias(alias) => {
            instance_type.alias(alias.into());
        }
        InstanceTypeDeclaration::Export { name, desc } => {
            instance_type.export(name.into(), desc.into());
        }
    }
    Ok(())
}

fn add_to_component_type(
    encoder: wasm_encoder::ComponentTypeEncoder,
    value: &ComponentType,
) -> Result<(), String> {
    match value {
        ComponentType::Defined(component_defined_type) => {
            let defined_type = encoder.defined_type();
            add_to_defined_type(defined_type, component_defined_type);
        }
        ComponentType::Func(component_func_type) => {
            let mut function = encoder.function();
            function.params(
                component_func_type
                    .params
                    .iter()
                    .map(|(name, val_type)| (name.as_str(), val_type)),
            );
            function.result(component_func_type.result.as_ref().map(|tpe| tpe.into()));
        }
        ComponentType::Component(component_type_declarations) => {
            let mut component_type = wasm_encoder::ComponentType::new();
            for component_type_declaration in &component_type_declarations.0 {
                add_declaration_to_component_type(&mut component_type, component_type_declaration)?;
            }
            encoder.component(&component_type);
        }
        ComponentType::Instance(instance_type_declarations) => {
            let mut instance_type = wasm_encoder::InstanceType::new();
            for instance_type_declaration in &instance_type_declarations.0 {
                add_declaration_to_instance_type(&mut instance_type, instance_type_declaration)?;
            }
            encoder.instance(&instance_type);
        }
        ComponentType::Resource {
            representation,
            destructor,
        } => encoder.resource(representation.into(), *destructor),
    }
    Ok(())
}

fn add_to_type_section(
    section: &mut wasm_encoder::ComponentTypeSection,
    value: &ComponentType,
) -> Result<(), String> {
    add_to_component_type(section.ty(), value)
}

impl From<&PrimitiveValueType> for wasm_encoder::PrimitiveValType {
    fn from(value: &PrimitiveValueType) -> Self {
        match value {
            PrimitiveValueType::Bool => wasm_encoder::PrimitiveValType::Bool,
            PrimitiveValueType::S8 => wasm_encoder::PrimitiveValType::S8,
            PrimitiveValueType::U8 => wasm_encoder::PrimitiveValType::U8,
            PrimitiveValueType::S16 => wasm_encoder::PrimitiveValType::S16,
            PrimitiveValueType::U16 => wasm_encoder::PrimitiveValType::U16,
            PrimitiveValueType::S32 => wasm_encoder::PrimitiveValType::S32,
            PrimitiveValueType::U32 => wasm_encoder::PrimitiveValType::U32,
            PrimitiveValueType::S64 => wasm_encoder::PrimitiveValType::S64,
            PrimitiveValueType::U64 => wasm_encoder::PrimitiveValType::U64,
            PrimitiveValueType::F32 => wasm_encoder::PrimitiveValType::F32,
            PrimitiveValueType::F64 => wasm_encoder::PrimitiveValType::F64,
            PrimitiveValueType::Chr => wasm_encoder::PrimitiveValType::Char,
            PrimitiveValueType::Str => wasm_encoder::PrimitiveValType::String,
            PrimitiveValueType::ErrorContext => wasm_encoder::PrimitiveValType::ErrorContext,
        }
    }
}

impl From<&ComponentValType> for wasm_encoder::ComponentValType {
    fn from(value: &ComponentValType) -> Self {
        match value {
            ComponentValType::Primitive(primitive_value_type) => {
                wasm_encoder::ComponentValType::Primitive(primitive_value_type.into())
            }
            ComponentValType::Defined(component_type_idx) => {
                wasm_encoder::ComponentValType::Type(*component_type_idx)
            }
        }
    }
}

impl<'a> From<&'a VariantCase> for (&'a str, Option<wasm_encoder::ComponentValType>, Option<u32>) {
    fn from(value: &'a VariantCase) -> Self {
        (
            &value.name,
            value.typ.as_ref().map(|t| t.into()),
            value.refines,
        )
    }
}

fn add_to_defined_type(
    defined_type: wasm_encoder::ComponentDefinedTypeEncoder,
    value: &ComponentDefinedType,
) {
    match value {
        ComponentDefinedType::Primitive { typ } => {
            defined_type.primitive(typ.into());
        }
        ComponentDefinedType::Record { fields } => {
            defined_type.record(fields.iter().map(|(name, typ)| (name.as_str(), typ)));
        }
        ComponentDefinedType::Variant { cases } => {
            defined_type.variant(cases.iter().map(|case| case.into()));
        }
        ComponentDefinedType::List { elem } => {
            defined_type.list(elem);
        }
        ComponentDefinedType::Tuple { elems } => {
            defined_type.tuple(elems.iter());
        }
        ComponentDefinedType::Flags { names } => {
            defined_type.flags(names.iter().map(|name| name.as_str()));
        }
        ComponentDefinedType::Enum { names } => {
            defined_type.enum_type(names.iter().map(|name| name.as_str()));
        }
        ComponentDefinedType::Option { typ } => {
            defined_type.option(typ);
        }
        ComponentDefinedType::Result { ok, err } => {
            defined_type.result(
                ok.as_ref().map(|t| t.into()),
                err.as_ref().map(|t| t.into()),
            );
        }
        ComponentDefinedType::Owned { type_idx } => {
            defined_type.own(*type_idx);
        }
        ComponentDefinedType::Borrowed { type_idx } => {
            defined_type.borrow(*type_idx);
        }
        ComponentDefinedType::Future { inner } => {
            defined_type.future(inner.as_ref().map(|t| t.into()));
        }
        ComponentDefinedType::Stream { inner } => {
            defined_type.stream(inner.as_ref().map(|t| t.into()));
        }
    };
}

impl From<&Canon> for wasm_encoder::CanonicalFunctionSection {
    fn from(value: &Canon) -> Self {
        let mut section = wasm_encoder::CanonicalFunctionSection::new();
        add_to_canonical_function_section(&mut section, value);
        section
    }
}

impl From<&CanonicalOption> for wasm_encoder::CanonicalOption {
    fn from(value: &CanonicalOption) -> Self {
        match value {
            CanonicalOption::Utf8 => wasm_encoder::CanonicalOption::UTF8,
            CanonicalOption::Utf16 => wasm_encoder::CanonicalOption::UTF16,
            CanonicalOption::CompactUtf16 => wasm_encoder::CanonicalOption::CompactUTF16,
            CanonicalOption::Memory(mem_idx) => wasm_encoder::CanonicalOption::Memory(*mem_idx),
            CanonicalOption::Realloc(func_idx) => wasm_encoder::CanonicalOption::Realloc(*func_idx),
            CanonicalOption::PostReturn(func_idx) => {
                wasm_encoder::CanonicalOption::PostReturn(*func_idx)
            }
            CanonicalOption::Async => wasm_encoder::CanonicalOption::Async,
            CanonicalOption::Callback(func_idx) => {
                wasm_encoder::CanonicalOption::Callback(*func_idx)
            }
        }
    }
}

fn add_to_canonical_function_section(
    section: &mut wasm_encoder::CanonicalFunctionSection,
    value: &Canon,
) {
    match value {
        Canon::Lift {
            func_idx,
            opts,
            function_type,
        } => {
            section.lift(*func_idx, *function_type, opts.iter().map(|opt| opt.into()));
        }
        Canon::Lower { func_idx, opts } => {
            section.lower(*func_idx, opts.iter().map(|opt| opt.into()));
        }
        Canon::ResourceNew { type_idx } => {
            section.resource_new(*type_idx);
        }
        Canon::ResourceDrop { type_idx } => {
            section.resource_drop(*type_idx);
        }
        Canon::ResourceRep { type_idx } => {
            section.resource_rep(*type_idx);
        }
    }
}

impl From<&ComponentImport> for wasm_encoder::ComponentImportSection {
    fn from(value: &ComponentImport) -> Self {
        let mut section = wasm_encoder::ComponentImportSection::new();
        add_to_component_import_section(&mut section, value);
        section
    }
}

fn add_to_component_import_section(
    section: &mut wasm_encoder::ComponentImportSection,
    value: &ComponentImport,
) {
    section.import((&value.name).into(), (&value.desc).into());
}

impl From<&ComponentExport> for wasm_encoder::ComponentExportSection {
    fn from(value: &ComponentExport) -> Self {
        let mut section = wasm_encoder::ComponentExportSection::new();
        add_to_component_export_section(&mut section, value);
        section
    }
}

fn add_to_component_export_section(
    section: &mut wasm_encoder::ComponentExportSection,
    value: &ComponentExport,
) {
    section.export(
        (&value.name).into(),
        (&value.kind).into(),
        value.idx,
        value.desc.as_ref().map(|type_ref| type_ref.into()),
    );
}

impl<Ast> TryFrom<Component<Ast>> for wasm_encoder::Component
where
    Ast: AstCustomization,
    Ast::Expr: RetainsInstructions,
    Ast::Data: Into<Data<Ast::Expr>>,
    Ast::Custom: Into<Custom>,
{
    type Error = String;

    fn try_from(value: Component<Ast>) -> Result<Self, Self::Error> {
        let mut component = wasm_encoder::Component::new();

        for (section_type, sections) in value.into_grouped() {
            match section_type {
                ComponentSectionType::Module => {
                    let inner_module = sections.first().unwrap().as_module();
                    let encoded_module = wasm_encoder::Module::try_from(inner_module.clone())?;
                    let nested_module = wasm_encoder::ModuleSection(&encoded_module);
                    component.section(&nested_module);
                }
                ComponentSectionType::CoreInstance => {
                    let mut section = wasm_encoder::InstanceSection::new();
                    for core_instance in sections {
                        add_to_core_instance_section(
                            &mut section,
                            core_instance.as_core_instance(),
                        );
                    }
                    component.section(&section);
                }
                ComponentSectionType::CoreType => {
                    let mut section = wasm_encoder::CoreTypeSection::new();
                    for core_type in sections {
                        add_to_core_type_section(&mut section, core_type.as_core_type())?;
                    }
                    component.section(&section);
                }
                ComponentSectionType::Component => {
                    let inner_component = sections.first().unwrap().as_component();
                    let encoded_component =
                        wasm_encoder::Component::try_from(inner_component.clone())?;
                    let nested_component = wasm_encoder::NestedComponentSection(&encoded_component);
                    component.section(&nested_component);
                }
                ComponentSectionType::Instance => {
                    let mut section = wasm_encoder::ComponentInstanceSection::new();
                    for component_instance in sections {
                        add_to_component_instance_section(
                            &mut section,
                            component_instance.as_instance(),
                        );
                    }
                    component.section(&section);
                }
                ComponentSectionType::Alias => {
                    let mut section = wasm_encoder::ComponentAliasSection::new();
                    for alias in sections {
                        add_to_alias_section(&mut section, alias.as_alias());
                    }
                    component.section(&section);
                }
                ComponentSectionType::Type => {
                    let mut section = wasm_encoder::ComponentTypeSection::new();
                    for typ in sections {
                        add_to_type_section(&mut section, typ.as_type())?;
                    }
                    component.section(&section);
                }
                ComponentSectionType::Canon => {
                    let mut section = wasm_encoder::CanonicalFunctionSection::new();
                    for canon in sections {
                        add_to_canonical_function_section(&mut section, canon.as_canon());
                    }
                    component.section(&section);
                }
                ComponentSectionType::Start => {
                    let start = sections.first().unwrap().as_start();
                    let section = wasm_encoder::ComponentStartSection {
                        function_index: start.func_idx,
                        args: start.args.clone(),
                        results: start.results,
                    };
                    component.section(&section);
                }
                ComponentSectionType::Import => {
                    let mut section = wasm_encoder::ComponentImportSection::new();
                    for import in sections {
                        add_to_component_import_section(&mut section, import.as_import());
                    }
                    component.section(&section);
                }
                ComponentSectionType::Export => {
                    let mut section = wasm_encoder::ComponentExportSection::new();
                    for export in sections {
                        add_to_component_export_section(&mut section, export.as_export());
                    }
                    component.section(&section);
                }
                ComponentSectionType::Custom => {
                    let custom = sections.first().unwrap().as_custom();
                    let custom: Custom = custom.clone().into();
                    let section: wasm_encoder::CustomSection = custom.into();
                    component.section(&section);
                }
            }
        }

        Ok(component)
    }
}
