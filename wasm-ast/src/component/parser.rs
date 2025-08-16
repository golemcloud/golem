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
use crate::core::{Data, TryFromExprSource};
use crate::Sections;
use wasmparser::{CanonicalFunction, Chunk, Parser, Payload};

impl TryFrom<wasmparser::InstantiationArg<'_>> for InstantiationArg {
    type Error = String;

    fn try_from(value: wasmparser::InstantiationArg) -> Result<Self, Self::Error> {
        let arg_ref = match value.kind {
            wasmparser::InstantiationArgKind::Instance => {
                InstantiationArgRef::Instance(value.index)
            }
        };
        Ok(InstantiationArg {
            name: value.name.to_string(),
            arg_ref,
        })
    }
}

impl TryFrom<wasmparser::Instance<'_>> for Instance {
    type Error = String;

    fn try_from(value: wasmparser::Instance) -> Result<Self, Self::Error> {
        match value {
            wasmparser::Instance::Instantiate { module_index, args } => Ok(Instance::Instantiate {
                module_idx: module_index,
                args: args
                    .iter()
                    .map(|arg| arg.clone().try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::Instance::FromExports(exports) => Ok(Instance::FromExports {
                exports: exports
                    .iter()
                    .map(|&export| export.try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
        }
    }
}

impl TryFrom<wasmparser::OuterAliasKind> for OuterAliasKind {
    type Error = String;

    fn try_from(value: wasmparser::OuterAliasKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::OuterAliasKind::Type => Ok(OuterAliasKind::Type),
        }
    }
}

impl TryFrom<wasmparser::ModuleTypeDeclaration<'_>> for ModuleDeclaration {
    type Error = String;

    fn try_from(value: wasmparser::ModuleTypeDeclaration) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ModuleTypeDeclaration::Type(recgroup) => {
                let subtype = recgroup.into_types().next().ok_or("Empty rec group")?;
                Ok(ModuleDeclaration::Type {
                    typ: subtype.try_into()?,
                })
            }
            wasmparser::ModuleTypeDeclaration::Export { name, ty } => {
                Ok(ModuleDeclaration::Export {
                    name: name.to_string(),
                    desc: ty.try_into()?,
                })
            }
            wasmparser::ModuleTypeDeclaration::OuterAlias { kind, count, index } => {
                Ok(ModuleDeclaration::OuterAlias {
                    kind: kind.try_into()?,
                    target: AliasTarget { count, index },
                })
            }
            wasmparser::ModuleTypeDeclaration::Import(import) => Ok(ModuleDeclaration::Import {
                import: import.try_into()?,
            }),
        }
    }
}

impl TryFrom<wasmparser::SubType> for FuncType {
    type Error = String;

    fn try_from(value: wasmparser::SubType) -> Result<Self, Self::Error> {
        if value.is_final {
            match value.composite_type.inner {
                wasmparser::CompositeInnerType::Func(func_type) => Ok(func_type.try_into()?),
                wasmparser::CompositeInnerType::Array(_) => {
                    Err("GC proposal is not supported".to_string())
                }
                wasmparser::CompositeInnerType::Struct(_) => {
                    Err("GC proposal is not supported".to_string())
                }
                wasmparser::CompositeInnerType::Cont(_) => {
                    Err("Task switching proposal is not supported".to_string())
                }
            }
        } else {
            Err("GC proposal is not supported".to_string())
        }
    }
}

impl TryFrom<wasmparser::CoreType<'_>> for CoreType {
    type Error = String;

    fn try_from(value: wasmparser::CoreType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::CoreType::Rec(recgroup) => {
                let subtype = recgroup.into_types().next().ok_or("Empty rec group")?;
                Ok(CoreType::Function(subtype.try_into()?))
            }
            wasmparser::CoreType::Module(module_type_decl) => Ok(CoreType::Module(
                module_type_decl
                    .iter()
                    .map(|module_decl| module_decl.clone().try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            )),
        }
    }
}

impl TryFrom<wasmparser::ComponentExternalKind> for ComponentExternalKind {
    type Error = String;

    fn try_from(value: wasmparser::ComponentExternalKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentExternalKind::Module => Ok(ComponentExternalKind::Module),
            wasmparser::ComponentExternalKind::Func => Ok(ComponentExternalKind::Func),
            wasmparser::ComponentExternalKind::Value => Ok(ComponentExternalKind::Value),
            wasmparser::ComponentExternalKind::Type => Ok(ComponentExternalKind::Type),
            wasmparser::ComponentExternalKind::Instance => Ok(ComponentExternalKind::Instance),
            wasmparser::ComponentExternalKind::Component => Ok(ComponentExternalKind::Component),
        }
    }
}

impl<'a> TryFrom<wasmparser::ComponentInstantiationArg<'a>> for ComponentInstantiationArg {
    type Error = String;

    fn try_from(value: wasmparser::ComponentInstantiationArg<'a>) -> Result<Self, Self::Error> {
        Ok(ComponentInstantiationArg {
            name: value.name.to_string(),
            kind: value.kind.try_into()?,
            idx: value.index,
        })
    }
}

impl TryFrom<wasmparser::ComponentExportName<'_>> for ComponentExternName {
    type Error = String;

    fn try_from(value: wasmparser::ComponentExportName) -> Result<Self, Self::Error> {
        Ok(ComponentExternName::Name(value.0.to_string()))
    }
}

impl TryFrom<wasmparser::PrimitiveValType> for PrimitiveValueType {
    type Error = String;

    fn try_from(value: wasmparser::PrimitiveValType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::PrimitiveValType::Bool => Ok(PrimitiveValueType::Bool),
            wasmparser::PrimitiveValType::S8 => Ok(PrimitiveValueType::S8),
            wasmparser::PrimitiveValType::U8 => Ok(PrimitiveValueType::U8),
            wasmparser::PrimitiveValType::S16 => Ok(PrimitiveValueType::S16),
            wasmparser::PrimitiveValType::U16 => Ok(PrimitiveValueType::U16),
            wasmparser::PrimitiveValType::S32 => Ok(PrimitiveValueType::S32),
            wasmparser::PrimitiveValType::U32 => Ok(PrimitiveValueType::U32),
            wasmparser::PrimitiveValType::S64 => Ok(PrimitiveValueType::S64),
            wasmparser::PrimitiveValType::U64 => Ok(PrimitiveValueType::U64),
            wasmparser::PrimitiveValType::F32 => Ok(PrimitiveValueType::F32),
            wasmparser::PrimitiveValType::F64 => Ok(PrimitiveValueType::F64),
            wasmparser::PrimitiveValType::Char => Ok(PrimitiveValueType::Chr),
            wasmparser::PrimitiveValType::String => Ok(PrimitiveValueType::Str),
            wasmparser::PrimitiveValType::ErrorContext => Ok(PrimitiveValueType::ErrorContext),
        }
    }
}

impl TryFrom<wasmparser::ComponentValType> for ComponentValType {
    type Error = String;

    fn try_from(value: wasmparser::ComponentValType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentValType::Primitive(primitive_val_type) => {
                Ok(ComponentValType::Primitive(primitive_val_type.try_into()?))
            }
            wasmparser::ComponentValType::Type(component_type_idx) => {
                Ok(ComponentValType::Defined(component_type_idx))
            }
        }
    }
}

impl TryFrom<wasmparser::TypeBounds> for TypeBounds {
    type Error = String;

    fn try_from(value: wasmparser::TypeBounds) -> Result<Self, Self::Error> {
        match value {
            wasmparser::TypeBounds::Eq(component_type_idx) => {
                Ok(TypeBounds::Eq(component_type_idx))
            }
            wasmparser::TypeBounds::SubResource => Ok(TypeBounds::SubResource),
        }
    }
}

impl TryFrom<wasmparser::ComponentTypeRef> for ComponentTypeRef {
    type Error = String;

    fn try_from(value: wasmparser::ComponentTypeRef) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentTypeRef::Module(module_idx) => {
                Ok(ComponentTypeRef::Module(module_idx))
            }
            wasmparser::ComponentTypeRef::Func(func_idx) => Ok(ComponentTypeRef::Func(func_idx)),
            wasmparser::ComponentTypeRef::Value(component_val_type) => {
                Ok(ComponentTypeRef::Val(component_val_type.try_into()?))
            }
            wasmparser::ComponentTypeRef::Type(type_bounds) => {
                Ok(ComponentTypeRef::Type(type_bounds.try_into()?))
            }
            wasmparser::ComponentTypeRef::Instance(instance_idx) => {
                Ok(ComponentTypeRef::Instance(instance_idx))
            }
            wasmparser::ComponentTypeRef::Component(component_idx) => {
                Ok(ComponentTypeRef::Component(component_idx))
            }
        }
    }
}

impl<'a> TryFrom<wasmparser::ComponentExport<'a>> for ComponentExport {
    type Error = String;

    fn try_from(value: wasmparser::ComponentExport<'a>) -> Result<Self, Self::Error> {
        Ok(ComponentExport {
            name: value.name.try_into()?,
            kind: value.kind.try_into()?,
            idx: value.index,
            desc: match value.ty {
                Some(ty) => Some(ty.try_into()?),
                None => None,
            },
        })
    }
}

impl TryFrom<wasmparser::ComponentInstance<'_>> for ComponentInstance {
    type Error = String;

    fn try_from(value: wasmparser::ComponentInstance) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentInstance::Instantiate {
                component_index,
                args,
            } => Ok(ComponentInstance::Instantiate {
                component_idx: component_index,
                args: args
                    .iter()
                    .map(|arg| arg.clone().try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::ComponentInstance::FromExports(exports) => {
                Ok(ComponentInstance::FromExports {
                    exports: exports
                        .iter()
                        .map(|export| export.clone().try_into())
                        .collect::<Result<Vec<_>, String>>()?,
                })
            }
        }
    }
}

impl TryFrom<wasmparser::ExternalKind> for ExportKind {
    type Error = String;

    fn try_from(value: wasmparser::ExternalKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ExternalKind::Func => Ok(ExportKind::Func),
            wasmparser::ExternalKind::Table => Ok(ExportKind::Table),
            wasmparser::ExternalKind::Memory => Ok(ExportKind::Mem),
            wasmparser::ExternalKind::Global => Ok(ExportKind::Global),
            wasmparser::ExternalKind::Tag => {
                Err("Exception handling proposal is not supported".to_string())
            }
        }
    }
}

impl TryFrom<wasmparser::ComponentOuterAliasKind> for OuterAliasKind {
    type Error = String;

    fn try_from(value: wasmparser::ComponentOuterAliasKind) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentOuterAliasKind::CoreModule => Ok(OuterAliasKind::CoreModule),
            wasmparser::ComponentOuterAliasKind::CoreType => Ok(OuterAliasKind::CoreType),
            wasmparser::ComponentOuterAliasKind::Type => Ok(OuterAliasKind::Type),
            wasmparser::ComponentOuterAliasKind::Component => Ok(OuterAliasKind::Component),
        }
    }
}

impl<'a> TryFrom<wasmparser::ComponentAlias<'a>> for Alias {
    type Error = String;

    fn try_from(value: wasmparser::ComponentAlias<'a>) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentAlias::InstanceExport {
                kind,
                instance_index,
                name,
            } => Ok(Alias::InstanceExport {
                kind: kind.try_into()?,
                instance_idx: instance_index,
                name: name.to_string(),
            }),
            wasmparser::ComponentAlias::CoreInstanceExport {
                kind,
                instance_index,
                name,
            } => Ok(Alias::CoreInstanceExport {
                kind: kind.try_into()?,
                instance_idx: instance_index,
                name: name.to_string(),
            }),
            wasmparser::ComponentAlias::Outer { kind, count, index } => Ok(Alias::Outer {
                kind: kind.try_into()?,
                target: AliasTarget { count, index },
            }),
        }
    }
}

impl TryFrom<wasmparser::VariantCase<'_>> for VariantCase {
    type Error = String;

    fn try_from(value: wasmparser::VariantCase) -> Result<Self, Self::Error> {
        Ok(VariantCase {
            name: value.name.to_string(),
            typ: match value.ty {
                Some(ty) => Some(ty.try_into()?),
                None => None,
            },
            refines: value.refines,
        })
    }
}

impl TryFrom<wasmparser::ComponentDefinedType<'_>> for ComponentDefinedType {
    type Error = String;

    fn try_from(value: wasmparser::ComponentDefinedType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentDefinedType::Primitive(primitive_val_type) => {
                Ok(ComponentDefinedType::Primitive {
                    typ: primitive_val_type.try_into()?,
                })
            }
            wasmparser::ComponentDefinedType::Record(fields) => Ok(ComponentDefinedType::Record {
                fields: fields
                    .iter()
                    .map(|&(name, typ)| typ.try_into().map(|t| (name.to_string(), t)))
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::ComponentDefinedType::Variant(cases) => Ok(ComponentDefinedType::Variant {
                cases: cases
                    .iter()
                    .map(|case| case.clone().try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::ComponentDefinedType::List(tpe) => Ok(ComponentDefinedType::List {
                elem: tpe.try_into()?,
            }),
            wasmparser::ComponentDefinedType::Tuple(types) => Ok(ComponentDefinedType::Tuple {
                elems: types
                    .iter()
                    .map(|tpe| (*tpe).try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::ComponentDefinedType::Flags(names) => Ok(ComponentDefinedType::Flags {
                names: names
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
            }),
            wasmparser::ComponentDefinedType::Enum(names) => Ok(ComponentDefinedType::Enum {
                names: names
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
            }),
            wasmparser::ComponentDefinedType::Option(tpe) => Ok(ComponentDefinedType::Option {
                typ: tpe.try_into()?,
            }),
            wasmparser::ComponentDefinedType::Result { ok, err } => {
                Ok(ComponentDefinedType::Result {
                    ok: match ok {
                        Some(tpe) => Some(tpe.try_into()?),
                        None => None,
                    },
                    err: match err {
                        Some(tpe) => Some(tpe.try_into()?),
                        None => None,
                    },
                })
            }
            wasmparser::ComponentDefinedType::Own(component_type_idx) => {
                Ok(ComponentDefinedType::Owned {
                    type_idx: component_type_idx,
                })
            }
            wasmparser::ComponentDefinedType::Borrow(component_type_idx) => {
                Ok(ComponentDefinedType::Borrowed {
                    type_idx: component_type_idx,
                })
            }
            wasmparser::ComponentDefinedType::Future(tpe) => Ok(ComponentDefinedType::Future {
                inner: tpe.map(|tpe| tpe.try_into()).transpose()?,
            }),
            wasmparser::ComponentDefinedType::Stream(tpe) => Ok(ComponentDefinedType::Stream {
                inner: tpe.map(|tpe| tpe.try_into()).transpose()?,
            }),
            wasmparser::ComponentDefinedType::FixedSizeList(_, _) => {
                Err("Fixed-size lists are not supported".to_string())
            }
        }
    }
}

impl TryFrom<wasmparser::ComponentFuncType<'_>> for ComponentFuncType {
    type Error = String;

    fn try_from(value: wasmparser::ComponentFuncType) -> Result<Self, Self::Error> {
        Ok(ComponentFuncType {
            params: value
                .params
                .iter()
                .map(|&(name, typ)| typ.try_into().map(|t| (name.to_string(), t)))
                .collect::<Result<Vec<_>, String>>()?,
            result: value.result.map(|ty| ty.try_into()).transpose()?,
        })
    }
}

impl TryFrom<wasmparser::ComponentImportName<'_>> for ComponentExternName {
    type Error = String;

    fn try_from(value: wasmparser::ComponentImportName) -> Result<Self, Self::Error> {
        Ok(ComponentExternName::Name(value.0.to_string()))
    }
}

impl TryFrom<wasmparser::ComponentImport<'_>> for ComponentImport {
    type Error = String;

    fn try_from(value: wasmparser::ComponentImport) -> Result<Self, Self::Error> {
        Ok(ComponentImport {
            name: value.name.try_into()?,
            desc: value.ty.try_into()?,
        })
    }
}

impl<'a> TryFrom<wasmparser::ComponentTypeDeclaration<'a>> for ComponentTypeDeclaration {
    type Error = String;

    fn try_from(value: wasmparser::ComponentTypeDeclaration<'a>) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentTypeDeclaration::CoreType(core_type) => {
                Ok(ComponentTypeDeclaration::Core(core_type.try_into()?))
            }
            wasmparser::ComponentTypeDeclaration::Type(tpe) => {
                Ok(ComponentTypeDeclaration::Type(tpe.try_into()?))
            }
            wasmparser::ComponentTypeDeclaration::Alias(alias) => {
                Ok(ComponentTypeDeclaration::Alias(alias.try_into()?))
            }
            wasmparser::ComponentTypeDeclaration::Import(import) => {
                Ok(ComponentTypeDeclaration::Import(import.try_into()?))
            }
            wasmparser::ComponentTypeDeclaration::Export { name, ty } => {
                Ok(ComponentTypeDeclaration::Export {
                    name: name.try_into()?,
                    desc: ty.try_into()?,
                })
            }
        }
    }
}

impl<'a> TryFrom<wasmparser::InstanceTypeDeclaration<'a>> for InstanceTypeDeclaration {
    type Error = String;

    fn try_from(value: wasmparser::InstanceTypeDeclaration<'a>) -> Result<Self, Self::Error> {
        match value {
            wasmparser::InstanceTypeDeclaration::CoreType(core_type) => {
                Ok(InstanceTypeDeclaration::Core(core_type.try_into()?))
            }
            wasmparser::InstanceTypeDeclaration::Type(tpe) => {
                Ok(InstanceTypeDeclaration::Type(tpe.try_into()?))
            }
            wasmparser::InstanceTypeDeclaration::Alias(alias) => {
                Ok(InstanceTypeDeclaration::Alias(alias.try_into()?))
            }
            wasmparser::InstanceTypeDeclaration::Export { name, ty } => {
                Ok(InstanceTypeDeclaration::Export {
                    name: name.try_into()?,
                    desc: ty.try_into()?,
                })
            }
        }
    }
}

impl TryFrom<wasmparser::ComponentType<'_>> for ComponentType {
    type Error = String;

    fn try_from(value: wasmparser::ComponentType) -> Result<Self, Self::Error> {
        match value {
            wasmparser::ComponentType::Defined(component_defined_type) => {
                Ok(ComponentType::Defined(component_defined_type.try_into()?))
            }
            wasmparser::ComponentType::Func(component_func_type) => {
                Ok(ComponentType::Func(component_func_type.try_into()?))
            }
            wasmparser::ComponentType::Component(component_type_decls) => {
                Ok(ComponentType::Component(ComponentTypeDeclarations(
                    component_type_decls
                        .iter()
                        .map(|component_type_decl| component_type_decl.clone().try_into())
                        .collect::<Result<Vec<_>, String>>()?,
                )))
            }
            wasmparser::ComponentType::Instance(instancetype_decls) => {
                Ok(ComponentType::Instance(InstanceTypeDeclarations(
                    instancetype_decls
                        .iter()
                        .map(|instancetype_decl| instancetype_decl.clone().try_into())
                        .collect::<Result<Vec<_>, String>>()?,
                )))
            }
            wasmparser::ComponentType::Resource { rep, dtor } => Ok(ComponentType::Resource {
                representation: rep.try_into()?,
                destructor: dtor,
            }),
        }
    }
}

impl TryFrom<wasmparser::CanonicalOption> for CanonicalOption {
    type Error = String;

    fn try_from(value: wasmparser::CanonicalOption) -> Result<Self, Self::Error> {
        match value {
            wasmparser::CanonicalOption::UTF8 => Ok(CanonicalOption::Utf8),
            wasmparser::CanonicalOption::UTF16 => Ok(CanonicalOption::Utf16),
            wasmparser::CanonicalOption::CompactUTF16 => Ok(CanonicalOption::CompactUtf16),
            wasmparser::CanonicalOption::Memory(mem_idx) => Ok(CanonicalOption::Memory(mem_idx)),
            wasmparser::CanonicalOption::Realloc(func_idx) => {
                Ok(CanonicalOption::Realloc(func_idx))
            }
            wasmparser::CanonicalOption::PostReturn(func_idx) => {
                Ok(CanonicalOption::PostReturn(func_idx))
            }
            wasmparser::CanonicalOption::Async => Ok(CanonicalOption::Async),
            wasmparser::CanonicalOption::Callback(func_idx) => {
                Ok(CanonicalOption::Callback(func_idx))
            }
            wasmparser::CanonicalOption::CoreType(_) => {
                Err("GC proposal is not supported".to_string())
            }
            wasmparser::CanonicalOption::Gc => Err("GC proposal is not supported".to_string()),
        }
    }
}

impl TryFrom<wasmparser::CanonicalFunction> for Canon {
    type Error = String;

    fn try_from(value: wasmparser::CanonicalFunction) -> Result<Self, Self::Error> {
        match value {
            wasmparser::CanonicalFunction::Lift {
                core_func_index,
                type_index,
                options,
            } => Ok(Canon::Lift {
                func_idx: core_func_index,
                function_type: type_index,
                opts: options
                    .iter()
                    .map(|&opt| opt.try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::CanonicalFunction::Lower {
                func_index,
                options,
            } => Ok(Canon::Lower {
                func_idx: func_index,
                opts: options
                    .iter()
                    .map(|&opt| opt.try_into())
                    .collect::<Result<Vec<_>, String>>()?,
            }),
            wasmparser::CanonicalFunction::ResourceNew { resource } => {
                Ok(Canon::ResourceNew { type_idx: resource })
            }
            wasmparser::CanonicalFunction::ResourceDrop { resource } => {
                Ok(Canon::ResourceDrop { type_idx: resource })
            }
            wasmparser::CanonicalFunction::ResourceRep { resource } => {
                Ok(Canon::ResourceRep { type_idx: resource })
            }
            CanonicalFunction::ThreadSpawnRef { .. } => {
                Err("Threads proposal is not supported".to_string())
            }
            CanonicalFunction::ResourceDropAsync { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ThreadAvailableParallelism => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::BackpressureSet => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::TaskReturn { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::Yield { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::SubtaskDrop => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamNew { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamRead { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamWrite { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamCancelRead { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamCancelWrite { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamDropReadable { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::StreamDropWritable { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureNew { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureRead { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureWrite { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureCancelRead { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureCancelWrite { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureDropReadable { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::FutureDropWritable { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ErrorContextNew { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ErrorContextDebugMessage { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ErrorContextDrop => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::WaitableSetNew => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::WaitableSetWait { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::WaitableSetPoll { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::WaitableSetDrop => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::WaitableJoin => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ThreadSpawnIndirect { .. } => {
                Err("Threads proposal is not supported".to_string())
            }
            CanonicalFunction::TaskCancel => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ContextGet(_) => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::ContextSet(_) => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
            CanonicalFunction::SubtaskCancel { .. } => {
                Err("WASI P3 future and stream support is not supported yet".to_string())
            }
        }
    }
}

impl TryFrom<wasmparser::ComponentStartFunction> for ComponentStart {
    type Error = String;

    fn try_from(value: wasmparser::ComponentStartFunction) -> Result<Self, Self::Error> {
        Ok(ComponentStart {
            func_idx: value.func_index,
            args: value.arguments.to_vec(),
            results: value.results,
        })
    }
}

#[allow(clippy::type_complexity)]
fn parse_component_sections<Ast>(
    mut parser: Parser,
    mut remaining: &[u8],
) -> Result<
    (
        Sections<ComponentIndexSpace, ComponentSectionType, ComponentSection<Ast>>,
        &[u8],
    ),
    String,
>
where
    Ast: AstCustomization,
    Ast::Expr: TryFromExprSource,
    Ast::Data: From<Data<Ast::Expr>>,
    Ast::Custom: From<Custom>,
{
    let mut sections = Vec::new();
    loop {
        let payload = match parser
            .parse(remaining, true)
            .map_err(|e| format!("Error parsing core module: {e:?}"))?
        {
            Chunk::Parsed { payload, consumed } => {
                remaining = &remaining[consumed..];
                payload
            }
            Chunk::NeedMoreData { .. } => {
                return Err("Unexpected end of component binary".to_string());
            }
        };
        match payload {
            Payload::Version { .. } => {}
            Payload::TypeSection(_) => {
                return Err("Unexpected core type section in component".to_string());
            }

            Payload::ImportSection(_) => {
                return Err("Unexpected core import section in component".to_string());
            }

            Payload::FunctionSection(_) => {
                return Err("Unexpected core function section in component".to_string());
            }

            Payload::TableSection(_) => {
                return Err("Unexpected core table section in component".to_string());
            }

            Payload::MemorySection(_) => {
                return Err("Unexpected core memory section in component".to_string());
            }

            Payload::TagSection(_) => {
                return Err("Unexpected core tag section in component".to_string());
            }
            Payload::GlobalSection(_) => {
                return Err("Unexpected core global section in component".to_string());
            }

            Payload::ExportSection(_) => {
                return Err("Unexpected core export section in component".to_string());
            }

            Payload::StartSection { .. } => {
                return Err("Unexpected core start section in component".to_string());
            }

            Payload::ElementSection(_) => {
                return Err("Unexpected core element section in component".to_string());
            }

            Payload::DataCountSection { .. } => {
                return Err("Unexpected core data count section in component".to_string());
            }

            Payload::DataSection(_) => {
                return Err("Unexpected core data section in component".to_string());
            }

            Payload::CodeSectionStart { .. } => {
                return Err("Unexpected core code section in component".to_string());
            }

            Payload::CodeSectionEntry(_) => {
                return Err("Unexpected core code section in component".to_string());
            }

            Payload::CustomSection(reader) => sections.push(ComponentSection::Custom(
                Custom {
                    name: reader.name().to_string(),
                    data: reader.data().to_vec(),
                }
                .into(),
            )),
            Payload::End(_) => {
                break;
            }
            Payload::InstanceSection(reader) => {
                for instance in reader {
                    let instance = instance.map_err(|e| {
                        format!("Error parsing component core instance section: {e:?}")
                    })?;
                    sections.push(ComponentSection::CoreInstance(instance.try_into()?))
                }
            }
            Payload::CoreTypeSection(reader) => {
                for core_type in reader {
                    let core_type = core_type
                        .map_err(|e| format!("Error parsing component core type section: {e:?}"))?;
                    sections.push(ComponentSection::CoreType(core_type.try_into()?))
                }
            }
            Payload::ModuleSection {
                parser,
                unchecked_range,
            } => {
                let module: Module<Ast> =
                    (parser, &remaining[..unchecked_range.len()]).try_into()?;
                remaining = &remaining[(unchecked_range.end - unchecked_range.start)..];
                sections.push(ComponentSection::Module(module))
            }
            Payload::ComponentSection { parser, .. } => {
                let (component, new_remaining) = parse_component(parser, remaining)?;
                remaining = new_remaining;
                sections.push(ComponentSection::Component(component))
            }
            Payload::ComponentInstanceSection(reader) => {
                for component_instance in reader {
                    let component_instance = component_instance
                        .map_err(|e| format!("Error parsing component instance section: {e:?}"))?;
                    sections.push(ComponentSection::Instance(component_instance.try_into()?))
                }
            }
            Payload::ComponentAliasSection(reader) => {
                for alias in reader {
                    let alias = alias
                        .map_err(|e| format!("Error parsing component alias section: {e:?}"))?;
                    sections.push(ComponentSection::Alias(alias.try_into()?))
                }
            }
            Payload::ComponentTypeSection(reader) => {
                for component_type in reader {
                    let component_type = component_type
                        .map_err(|e| format!("Error parsing component type section: {e:?}"))?;
                    sections.push(ComponentSection::Type(component_type.try_into()?))
                }
            }
            Payload::ComponentCanonicalSection(reader) => {
                for canon in reader {
                    let canon = canon
                        .map_err(|e| format!("Error parsing component canonical section: {e:?}"))?;
                    sections.push(ComponentSection::Canon(canon.try_into()?))
                }
            }
            Payload::ComponentStartSection { start, .. } => {
                sections.push(ComponentSection::Start(start.try_into()?))
            }
            Payload::ComponentImportSection(reader) => {
                for import in reader {
                    let import = import
                        .map_err(|e| format!("Error parsing component import section: {e:?}"))?;
                    sections.push(ComponentSection::Import(import.try_into()?))
                }
            }
            Payload::ComponentExportSection(reader) => {
                for export in reader {
                    let export = export
                        .map_err(|e| format!("Error parsing component export section: {e:?}"))?;
                    sections.push(ComponentSection::Export(export.try_into()?))
                }
            }
            Payload::UnknownSection { .. } => {
                return Err("Unexpected unknown section in component".to_string());
            }
            _ => {
                return Err("Unexpected section in component".to_string());
            }
        }
    }

    Ok((Sections::from_flat(sections), remaining))
}

pub fn parse_component<Ast>(
    parser: Parser,
    remaining: &[u8],
) -> Result<(Component<Ast>, &[u8]), String>
where
    Ast: AstCustomization,
    Ast::Expr: TryFromExprSource,
    Ast::Data: From<Data<Ast::Expr>>,
    Ast::Custom: From<Custom>,
{
    let (sections, remaining) = parse_component_sections(parser, remaining)?;
    Ok((sections.into(), remaining))
}
