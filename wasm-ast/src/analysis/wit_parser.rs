use crate::analysis::{
    analysed_type, AnalysedExport, AnalysedFunction, AnalysedFunctionParameter,
    AnalysedFunctionResult, AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
    AnalysedType, AnalysisFailure, AnalysisResult, NameOptionTypePair, NameTypePair, TypeHandle,
};
use itertools::Itertools;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use wit_parser::decoding::DecodedWasm;
use wit_parser::{
    Function, Handle, Interface, PackageName, Resolve, Type, TypeDef, TypeDefKind, WorldItem,
};
use wit_parser::{TypeId, TypeOwner as WitParserTypeOwner};

pub struct WitAnalysisContext {
    wasm: DecodedWasm,
    resource_ids: Rc<RefCell<HashMap<TypeId, AnalysedResourceId>>>,
}

impl WitAnalysisContext {
    pub fn new(component_bytes: &[u8]) -> AnalysisResult<WitAnalysisContext> {
        let wasm = wit_parser::decoding::decode(component_bytes).map_err(|err| {
            AnalysisFailure::failed(format!("Failed to decode WASM component: {err:#}"))
        })?;
        Ok(Self {
            wasm,
            resource_ids: Rc::new(RefCell::new(HashMap::new())),
        })
    }

    /// Get all top-level exports from the component with all the type information gathered from
    /// the component AST.
    pub fn get_top_level_exports(&self) -> AnalysisResult<Vec<AnalysedExport>> {
        let package_id = self.wasm.package();
        let resolve = self.wasm.resolve();

        let root_package =
            AnalysisFailure::fail_on_missing(resolve.packages.get(package_id), "root package")?;

        if root_package.worlds.len() > 1 {
            Err(AnalysisFailure::failed(
                "The component's root package must contains a single world",
            ))
        } else if root_package.worlds.is_empty() {
            Err(AnalysisFailure::failed(
                "The component's root package must contain a world",
            ))
        } else {
            let (_world_name, world_id) = root_package.worlds.iter().next().unwrap();
            let world = AnalysisFailure::fail_on_missing(resolve.worlds.get(*world_id), "world")?;

            let mut result = Vec::new();
            for (_world_key, world_item) in &world.exports {
                match world_item {
                    WorldItem::Interface { id, .. } => {
                        let interface = AnalysisFailure::fail_on_missing(
                            resolve.interfaces.get(*id),
                            "interface",
                        )?;

                        result.push(AnalysedExport::Instance(self.analyse_interface(interface)?))
                    }
                    WorldItem::Function(function) => {
                        result.push(AnalysedExport::Function(self.analyse_function(function)?));
                    }
                    WorldItem::Type(_) => {}
                }
            }

            Ok(result)
        }
    }

    pub fn root_package_name(&self) -> AnalysisResult<PackageName> {
        let package_id = self.wasm.package();
        let resolve = self.wasm.resolve();

        let root_package =
            AnalysisFailure::fail_on_missing(resolve.packages.get(package_id), "root package")?;

        Ok(root_package.name.clone())
    }

    /// Gets a binary WIT representation of the component's interface
    pub fn serialized_interface_only(&self) -> AnalysisResult<Vec<u8>> {
        let decoded_package = self.wasm.package();
        let bytes = wit_component::encode(self.wasm.resolve(), decoded_package).map_err(|err| {
            AnalysisFailure::failed(format!(
                "Failed to encode WASM component interface: {err:#}"
            ))
        })?;
        Ok(bytes)
    }

    fn analyse_function(&self, function: &Function) -> AnalysisResult<AnalysedFunction> {
        Ok(AnalysedFunction {
            name: function.name.clone(),
            parameters: function
                .params
                .iter()
                .map(|(name, typ)| {
                    typ.to_analysed_type(self.wasm.resolve(), self)
                        .map_err(|err| {
                            AnalysisFailure::failed(format!(
                                "Failed to decode function ({}) parameter ({}) type: {}",
                                function.name, name, err
                            ))
                        })
                        .map(|typ| AnalysedFunctionParameter {
                            name: name.clone(),
                            typ,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?,
            results: function
                .result
                .map(|typ| {
                    typ.to_analysed_type(self.wasm.resolve(), self)
                        .map_err(|err| {
                            AnalysisFailure::failed(format!(
                                "Failed to decode function ({}) result type: {}",
                                function.name, err
                            ))
                        })
                        .map(|typ| AnalysedFunctionResult { name: None, typ })
                })
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn analyse_interface(&self, interface: &Interface) -> AnalysisResult<AnalysedInstance> {
        let mut functions = Vec::new();
        for (_, function) in &interface.functions {
            functions.push(self.analyse_function(function)?);
        }
        let interface_name =
            AnalysisFailure::fail_on_missing(interface.name.clone(), "interface name")?;
        let package_id = AnalysisFailure::fail_on_missing(interface.package, "interface package")?;
        let package = AnalysisFailure::fail_on_missing(
            self.wasm.resolve().packages.get(package_id),
            "interface package",
        )?;

        Ok(AnalysedInstance {
            name: package.name.interface_id(&interface_name),
            functions,
        })
    }
}

impl GetResourceId for WitAnalysisContext {
    fn get_resource_id(&self, type_id: TypeId) -> Option<AnalysedResourceId> {
        let new_unique_id = self.resource_ids.borrow().len() as u64;
        let mut resource_ids = self.resource_ids.borrow_mut();

        Some(
            resource_ids
                .entry(type_id)
                .or_insert_with(|| AnalysedResourceId(new_unique_id))
                .clone(),
        )
    }
}

pub trait GetResourceId {
    fn get_resource_id(&self, type_id: TypeId) -> Option<AnalysedResourceId>;
}

pub struct ResourcesNotSupported;

impl GetResourceId for ResourcesNotSupported {
    fn get_resource_id(&self, _type_id: TypeId) -> Option<AnalysedResourceId> {
        None
    }
}

/// ToAnalysedType converts a Type or TypeDef from a wit_parser::Resolve.
///
/// ToAnalysedType is intended to be used for helping with writing tests where AnalysedType
/// have to be constructed in the test. For simpler values and types this is usually
/// not a problem, but creating more complex nested or variant types manually can be convoluted.
///
/// Note that resources and handles are not implemented.
pub trait ToAnalysedType {
    fn to_analysed_type(
        &self,
        resolve: &Resolve,
        resource_map: &impl GetResourceId,
    ) -> Result<AnalysedType, String>;
}

impl ToAnalysedType for TypeDef {
    fn to_analysed_type(
        &self,
        resolve: &Resolve,
        resource_map: &impl GetResourceId,
    ) -> Result<AnalysedType, String> {
        match &self.kind {
            TypeDefKind::Record(record) => Ok(analysed_type::record(
                record
                    .fields
                    .iter()
                    .map(|field| {
                        field
                            .ty
                            .to_analysed_type(resolve, resource_map)
                            .map(|typ| NameTypePair {
                                name: field.name.clone(),
                                typ,
                            })
                    })
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Resource => {
                Err("to_analysed_type not implemented for resource type".to_string())
            }

            TypeDefKind::Handle(handle) => match handle {
                Handle::Own(type_id) => match resource_map.get_resource_id(*type_id) {
                    Some(resource_id) => Ok(AnalysedType::Handle(TypeHandle {
                        resource_id,
                        mode: AnalysedResourceMode::Owned,
                    })),
                    None => Err("to_analysed_type not implemented for handle type".to_string()),
                },
                Handle::Borrow(type_id) => match resource_map.get_resource_id(*type_id) {
                    Some(resource_id) => Ok(AnalysedType::Handle(TypeHandle {
                        resource_id,
                        mode: AnalysedResourceMode::Borrowed,
                    })),
                    None => Err("to_analysed_type not implemented for handle type".to_string()),
                },
            },
            TypeDefKind::Flags(flag) => Ok(analysed_type::flags(
                &flag
                    .flags
                    .iter()
                    .map(|flag| flag.name.as_str())
                    .collect::<Vec<_>>(),
            )),
            TypeDefKind::Tuple(tuple) => Ok(analysed_type::tuple(
                tuple
                    .types
                    .iter()
                    .map(|typ| typ.to_analysed_type(resolve, resource_map))
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Variant(variant) => Ok(analysed_type::variant(
                variant
                    .cases
                    .iter()
                    .map(|case| {
                        case.ty
                            .map(|ty| ty.to_analysed_type(resolve, resource_map))
                            .transpose()
                            .map(|ty| NameOptionTypePair {
                                name: case.name.clone(),
                                typ: ty,
                            })
                    })
                    .collect::<Result<_, _>>()?,
            )),
            TypeDefKind::Enum(enum_) => Ok(analysed_type::r#enum(
                &enum_
                    .cases
                    .iter()
                    .map(|case| case.name.as_str())
                    .collect::<Vec<_>>(),
            )),
            TypeDefKind::Option(inner) => Ok(analysed_type::option(
                inner.to_analysed_type(resolve, resource_map)?,
            )),
            TypeDefKind::Result(result) => match (result.ok, result.err) {
                (Some(ok), Some(err)) => Ok(analysed_type::result(
                    ok.to_analysed_type(resolve, resource_map)?,
                    err.to_analysed_type(resolve, resource_map)?,
                )),
                (Some(ok), None) => Ok(analysed_type::result_ok(
                    ok.to_analysed_type(resolve, resource_map)?,
                )),
                (None, Some(err)) => Ok(analysed_type::result_err(
                    err.to_analysed_type(resolve, resource_map)?,
                )),
                (None, None) => Err("result type with no ok or err case".to_string()),
            },
            TypeDefKind::List(ty) => Ok(analysed_type::list(
                ty.to_analysed_type(resolve, resource_map)?,
            )),
            TypeDefKind::Future(_) => {
                Err("to_analysed_type not implemented for future type".to_string())
            }
            TypeDefKind::Stream(_) => {
                Err("to_analysed_type not implemented for stream type".to_string())
            }
            TypeDefKind::Type(typ) => typ.to_analysed_type(resolve, resource_map),
            TypeDefKind::Unknown => Err("to_analysed_type unknown type".to_string()),
        }
    }
}

impl ToAnalysedType for Type {
    fn to_analysed_type(
        &self,
        resolve: &Resolve,
        resource_map: &impl GetResourceId,
    ) -> Result<AnalysedType, String> {
        match self {
            Type::Bool => Ok(analysed_type::bool()),
            Type::U8 => Ok(analysed_type::u8()),
            Type::U16 => Ok(analysed_type::u16()),
            Type::U32 => Ok(analysed_type::u32()),
            Type::U64 => Ok(analysed_type::u64()),
            Type::S8 => Ok(analysed_type::s8()),
            Type::S16 => Ok(analysed_type::s16()),
            Type::S32 => Ok(analysed_type::s32()),
            Type::S64 => Ok(analysed_type::s64()),
            Type::F32 => Ok(analysed_type::f32()),
            Type::F64 => Ok(analysed_type::f64()),
            Type::Char => Ok(analysed_type::chr()),
            Type::String => Ok(analysed_type::str()),
            Type::Id(id) => resolve
                .types
                .get(*id)
                .ok_or_else(|| format!("Type not found by id: {:?}", id))?
                .to_analysed_type(resolve, resource_map),
            Type::ErrorContext => Err("ErrorContext not supported".to_string()),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum TypeOwner {
    World(String),
    Interface(String),
    InlineInterface,
    None,
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct TypeName {
    pub package: Option<String>,
    pub owner: TypeOwner,
    pub name: Option<String>,
}

pub struct AnalysedTypeResolve {
    resolve: Resolve,
    type_name_to_id: HashMap<TypeName, TypeId>,
    id_to_analysed_type: HashMap<TypeId, AnalysedType>,
}

impl Debug for AnalysedTypeResolve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnalysedTypeResolve")
    }
}

impl AnalysedTypeResolve {
    pub fn new(resolve: Resolve) -> Self {
        let type_name_to_id = resolve
            .types
            .iter()
            .map(|(type_id, type_def)| {
                (
                    match &type_def.owner {
                        WitParserTypeOwner::World(world_id) => {
                            let world = resolve.worlds.get(*world_id).unwrap();
                            TypeName {
                                package: world
                                    .package
                                    .and_then(|package_id| resolve.packages.get(package_id))
                                    .map(|package| package.name.to_string()),
                                owner: TypeOwner::World(world.name.clone()),
                                name: type_def.name.clone(),
                            }
                        }
                        WitParserTypeOwner::Interface(interface_id) => {
                            let interface = resolve.interfaces.get(*interface_id).unwrap();
                            TypeName {
                                package: interface
                                    .package
                                    .and_then(|package_id| resolve.packages.get(package_id))
                                    .map(|package| package.name.to_string()),
                                owner: {
                                    match &interface.name {
                                        Some(name) => TypeOwner::Interface(name.clone()),
                                        None => TypeOwner::InlineInterface,
                                    }
                                },
                                name: type_def.name.clone(),
                            }
                        }
                        WitParserTypeOwner::None => TypeName {
                            package: None,
                            owner: TypeOwner::None,
                            name: type_def.name.clone(),
                        },
                    },
                    type_id,
                )
            })
            .collect::<HashMap<_, _>>();

        AnalysedTypeResolve {
            resolve,
            type_name_to_id,
            id_to_analysed_type: HashMap::new(),
        }
    }

    pub fn from_wit_directory(directory: &Path) -> Result<Self, String> {
        let mut resolve = Resolve::new();
        resolve.push_dir(directory).map_err(|e| e.to_string())?;
        Ok(Self::new(resolve))
    }

    pub fn from_wit_str(wit: &str) -> Result<Self, String> {
        let mut resolve = Resolve::new();
        resolve
            .push_str(wit, "wit.wit")
            .map_err(|e| e.to_string())?;
        Ok(Self::new(resolve))
    }

    pub fn analysed_type(&mut self, name: &TypeName) -> Result<AnalysedType, String> {
        match self.type_name_to_id.get(name) {
            Some(type_id) => match self.id_to_analysed_type.get(type_id) {
                Some(typ) => Ok(typ.clone()),
                None => {
                    let typ = self
                        .resolve
                        .types
                        .get(*type_id)
                        .unwrap()
                        .to_analysed_type(&self.resolve, &ResourcesNotSupported)?;
                    self.id_to_analysed_type.insert(*type_id, typ.clone());
                    Ok(typ)
                }
            },
            None => Err(format!(
                "Type not found by name: {:?}, available types: {}",
                name,
                {
                    self.type_name_to_id
                        .keys()
                        .map(|type_id| format!("{type_id:?}"))
                        .join("\n")
                }
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SharedAnalysedTypeResolve {
    resolve: Arc<Mutex<AnalysedTypeResolve>>,
}

impl SharedAnalysedTypeResolve {
    pub fn new(resolve: AnalysedTypeResolve) -> Self {
        Self {
            resolve: Arc::new(Mutex::new(resolve)),
        }
    }

    pub fn analysed_type(&mut self, name: &TypeName) -> Result<AnalysedType, String> {
        self.resolve.lock().unwrap().analysed_type(name)
    }
}
