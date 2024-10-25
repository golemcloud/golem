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

use crate::wit_resolve::ResolvedWitDir;
use crate::{naming, WasmRpcOverride};
use anyhow::anyhow;
use indexmap::IndexMap;
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use wit_parser::{
    Function, FunctionKind, Interface, InterfaceId, Package, PackageId, PackageName, Resolve,
    Results, Type, TypeDef, TypeDefKind, TypeId, TypeOwner, World, WorldId, WorldItem, WorldKey,
};

/// All the gathered information for generating the stub crate.
pub struct StubDefinition {
    resolve: Resolve,
    source_world_id: WorldId,
    sources: IndexMap<PackageId, (PathBuf, Vec<PathBuf>)>,
    stub_exported_interfaces: OnceCell<Vec<InterfaceStub>>,

    pub source_package_name: PackageName,
    pub source_wit_root: PathBuf,
    pub target_root: PathBuf,
    pub stub_crate_version: String,
    pub wasm_rpc_override: WasmRpcOverride,
    pub always_inline_types: bool,
}

impl StubDefinition {
    pub fn new(
        source_wit_root: &Path,
        target_root: &Path,
        selected_world: &Option<String>,
        stub_crate_version: &str,
        wasm_rpc_override: &WasmRpcOverride,
        always_inline_types: bool,
    ) -> anyhow::Result<Self> {
        let resolved_source = ResolvedWitDir::new(source_wit_root)?;

        let source_world_id = resolved_source
            .resolve
            .select_world(resolved_source.package_id, selected_world.as_deref())?;

        let source_package_name = resolved_source
            .resolve
            .packages
            .get(resolved_source.package_id)
            .unwrap_or_else(|| {
                panic!(
                    "Failed to get package by id: {:?}",
                    resolved_source.package_id
                )
            })
            .name
            .clone();

        Ok(Self {
            resolve: resolved_source.resolve,
            source_world_id,
            sources: resolved_source.sources,
            stub_exported_interfaces: OnceCell::new(),
            source_package_name,
            source_wit_root: source_wit_root.to_path_buf(),
            target_root: target_root.to_path_buf(),
            stub_crate_version: stub_crate_version.to_string(),
            wasm_rpc_override: wasm_rpc_override.clone(),
            always_inline_types,
        })
    }

    // NOTE: In the following "getters" below we trust that getting entities from the resolver
    //       by source_package_id and world_id must always succeed, given
    //         - they were created by using the resolver
    //         - the resolver is private
    //         - we do not allow mutation of the resolver or the ids
    //
    //       Because of the above, we panic in case they are not found in the resolver,
    //       and similar reasoning is used when in one function we get and resolve an ID, since
    //       these would be internal arena errors.

    pub fn packages_with_wit_sources(
        &self,
    ) -> impl Iterator<Item = (&Package, &(PathBuf, Vec<PathBuf>))> {
        self.resolve
            .topological_packages()
            .into_iter()
            .map(|package_id| {
                (
                    self.resolve
                        .packages
                        .get(package_id)
                        .unwrap_or_else(|| panic!("package not found")),
                    self.sources
                        .get(&package_id)
                        .unwrap_or_else(|| panic!("sources for package not found")),
                )
            })
    }

    pub fn stub_package_name(&self) -> PackageName {
        naming::wit::stub_package_name(&self.source_package_name)
    }

    pub fn source_world(&self) -> &World {
        self.resolve
            .worlds
            .get(self.source_world_id)
            .unwrap_or_else(|| panic!("selected world not found"))
    }

    pub fn source_world_name(&self) -> String {
        self.source_world().name.clone()
    }

    pub fn target_cargo_path(&self) -> PathBuf {
        self.target_root.join("Cargo.toml")
    }

    pub fn target_crate_name(&self) -> String {
        format!("{}-stub", self.source_world_name())
    }

    pub fn target_rust_path(&self) -> PathBuf {
        self.target_root.join("src/lib.rs")
    }

    pub fn target_world_name(&self) -> String {
        format!("wasm-rpc-stub-{}", self.source_world_name())
    }

    pub fn target_wit_root(&self) -> PathBuf {
        self.target_root.join(naming::wit::WIT_DIR)
    }

    pub fn target_wit_path(&self) -> PathBuf {
        self.target_wit_root().join(naming::wit::STUB_WIT_FILE_NAME)
    }

    pub fn resolve_target_wit(&self) -> anyhow::Result<ResolvedWitDir> {
        ResolvedWitDir::new(&self.target_wit_root())
    }

    pub fn fix_inlined_owner(&self, typedef: &TypeDef) -> StubTypeOwner {
        if self.is_inlined(typedef) {
            StubTypeOwner::StubInterface
        } else {
            StubTypeOwner::Source(typedef.owner)
        }
    }

    fn is_inlined(&self, typedef: &TypeDef) -> bool {
        match &typedef.owner {
            TypeOwner::Interface(interface_id) => {
                if self.always_inline_types {
                    if let Some(resolved_owner_interface) =
                        self.resolve.interfaces.get(*interface_id)
                    {
                        if let Some(name) = resolved_owner_interface.name.as_ref() {
                            self.stub_exported_interfaces()
                                .iter()
                                .any(|interface| &interface.name == name)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            TypeOwner::World(_) => true,
            TypeOwner::None => false,
        }
    }

    pub fn stub_exported_interfaces(&self) -> &Vec<InterfaceStub> {
        self.stub_exported_interfaces.get_or_init(|| {
            let WorldItemsByType {
                types,
                functions,
                interfaces,
            } = self.partition_world_items(&self.source_world().exports);

            interfaces
                .into_iter()
                .flat_map(|(name, interface)| self.interface_to_stub_interfaces(name, interface))
                .chain(self.global_stub_interface(types, functions))
                .collect::<Vec<_>>()
        })
    }

    fn partition_world_items<'a>(
        &'a self,
        world_items: &'a IndexMap<WorldKey, WorldItem>,
    ) -> WorldItemsByType {
        let mut types = Vec::<(String, TypeId)>::new();
        let mut functions = Vec::new();
        let mut interfaces = Vec::new();

        for (world_key, world_item) in world_items {
            match world_item {
                WorldItem::Interface { id, stability: _ } => {
                    let interface =
                        self.resolve.interfaces.get(*id).unwrap_or_else(|| {
                            panic!("failed to resolve interface by id, {:?}", id)
                        });
                    interfaces.push((
                        interface
                            .name
                            .clone()
                            .unwrap_or(String::from(world_key.clone())),
                        interface,
                    ))
                }
                WorldItem::Function(function) => functions.push(function),
                WorldItem::Type(type_id) => types.push((world_key.clone().unwrap_name(), *type_id)),
            }
        }

        WorldItemsByType {
            types,
            functions,
            interfaces,
        }
    }

    fn function_to_stub(function: &Function) -> FunctionStub {
        let name = function.name.clone();

        let params = function
            .params
            .iter()
            .map(|(name, typ)| FunctionParamStub {
                name: name.clone(),
                typ: *typ,
            })
            .collect();

        let results = match &function.results {
            Results::Named(params) => {
                let mut param_stubs = Vec::new();
                for (name, typ) in params {
                    param_stubs.push(FunctionParamStub {
                        name: name.clone(),
                        typ: *typ,
                    });
                }
                FunctionResultStub::Multi(param_stubs)
            }
            Results::Anon(single) => FunctionResultStub::Single(*single),
        };

        FunctionStub {
            name,
            params,
            results,
        }
    }

    fn functions_to_stubs<'a>(functions: impl Iterator<Item = &'a Function>) -> Vec<FunctionStub> {
        functions.map(Self::function_to_stub).collect()
    }

    fn global_stub_interface(
        &self,
        types: Vec<(String, TypeId)>,
        functions: Vec<&Function>,
    ) -> Option<InterfaceStub> {
        if functions.is_empty() {
            return None;
        }

        let name = self.source_world_name();

        let functions = Self::functions_to_stubs(
            functions
                .into_iter()
                .filter(|function| function.kind == FunctionKind::Freestanding),
        );

        let (imported_interfaces, _) = self.extract_interface_stubs_from_types(types.into_iter());

        Some(InterfaceStub {
            name,
            functions,
            imports: imported_interfaces,
            global: true,
            constructor_params: None,
            static_functions: vec![],
            owner_interface: None,
        })
    }

    fn interface_to_stub_interfaces(
        &self,
        name: String,
        interface: &Interface,
    ) -> Vec<InterfaceStub> {
        let functions = Self::functions_to_stubs(interface.functions.values());

        let (imported_interfaces, resource_interfaces) = self.extract_interface_stubs_from_types(
            interface
                .types
                .iter()
                .map(|(name, typ)| (name.clone(), *typ)),
        );

        let mut interface_stubs = Vec::with_capacity(1 + resource_interfaces.len());

        interface_stubs.push(InterfaceStub {
            name,
            functions,
            imports: imported_interfaces,
            global: false,
            constructor_params: None,
            static_functions: vec![],
            owner_interface: None,
        });
        interface_stubs.extend(resource_interfaces);

        interface_stubs
    }

    fn extract_interface_stubs_from_types(
        &self,
        types: impl Iterator<Item = (String, TypeId)>,
    ) -> (Vec<InterfaceStubTypeDef>, Vec<InterfaceStub>) {
        let mut imported_interfaces = Vec::<InterfaceStubTypeDef>::new();
        let mut resource_interfaces = Vec::<InterfaceStub>::new();

        for (type_name, type_id) in types {
            let type_def = self
                .resolve
                .types
                .get(type_id)
                .unwrap_or_else(|| panic!("type {type_id:?} not found"));

            if let TypeOwner::Interface(owner_interface_id) = type_def.owner {
                let owner_interface = self
                    .resolve
                    .interfaces
                    .get(owner_interface_id)
                    .unwrap_or_else(|| panic!("interface {owner_interface_id:?} not found"));

                if type_def.kind == TypeDefKind::Resource {
                    resource_interfaces.push(self.resource_interface_stub(
                        owner_interface,
                        type_name,
                        type_id,
                    ))
                } else {
                    imported_interfaces.push(self.type_def_to_stub(
                        owner_interface,
                        type_name,
                        type_def.clone(),
                    ));
                }
            }
        }

        (imported_interfaces, resource_interfaces)
    }

    fn type_def_to_stub(
        &self,
        owner_interface: &Interface,
        type_name: String,
        type_def: TypeDef,
    ) -> InterfaceStubTypeDef {
        let package = owner_interface
            .package
            .and_then(|id| self.resolve.packages.get(id));

        let interface_name = owner_interface
            .name
            .clone()
            .unwrap_or_else(|| panic!("Failed to get owner interface name"));

        let interface_path = package
            .map(|p| p.name.interface_id(&interface_name))
            .unwrap_or(interface_name);

        InterfaceStubTypeDef {
            name: type_name,
            path: interface_path,
            package_name: package.map(|p| p.name.clone()),
            type_def,
        }
    }

    fn resource_interface_stub(
        &self,
        owner_interface: &Interface,
        type_name: String,
        type_id: TypeId,
    ) -> InterfaceStub {
        let functions_by_kind = |kind: FunctionKind| {
            owner_interface
                .functions
                .values()
                .filter(move |function| function.kind == kind)
        };

        let function_stubs_by_kind =
            |kind: FunctionKind| Self::functions_to_stubs(functions_by_kind(kind));

        let (imported_interfaces, _) = self.extract_interface_stubs_from_types(
            owner_interface
                .types
                .iter()
                .map(|(name, type_id)| (name.clone(), *type_id)),
        );

        let constructor_params = functions_by_kind(FunctionKind::Constructor(type_id))
            .next()
            .map(|c| {
                c.params
                    .iter()
                    .map(|(n, t)| FunctionParamStub {
                        name: n.clone(),
                        typ: *t,
                    })
                    .collect::<Vec<_>>()
            });

        InterfaceStub {
            name: type_name,
            functions: function_stubs_by_kind(FunctionKind::Method(type_id)),
            imports: imported_interfaces,
            global: false,
            constructor_params,
            static_functions: function_stubs_by_kind(FunctionKind::Static(type_id)),
            owner_interface: Some(owner_interface.name.clone().unwrap_or_else(|| {
                panic!(
                    "failed to get interface name for interface: {:?}",
                    owner_interface
                )
            })),
        }
    }

    // NOTE: Getters below are not trusted, as they could return ids from
    //       a different Resolver and arena, so these are returning anyhow errors

    pub fn get_package(&self, package_id: PackageId) -> anyhow::Result<&Package> {
        self.resolve
            .packages
            .get(package_id)
            .ok_or_else(|| anyhow!("failed to get package by id: {:?}", package_id))
    }

    pub fn get_interface(&self, interface_id: InterfaceId) -> anyhow::Result<&Interface> {
        self.resolve
            .interfaces
            .get(interface_id)
            .ok_or_else(|| anyhow!("failed to get interface by id: {:?}", interface_id))
    }

    pub fn get_world(&self, world_id: WorldId) -> anyhow::Result<&World> {
        self.resolve
            .worlds
            .get(world_id)
            .ok_or_else(|| anyhow!("failed to get world by id: {:?}", world_id))
    }

    pub fn get_type_def(&self, type_id: TypeId) -> anyhow::Result<&TypeDef> {
        self.resolve
            .types
            .get(type_id)
            .ok_or_else(|| anyhow!("failed to get type by id: {:?}", type_id))
    }
}

struct WorldItemsByType<'a> {
    pub types: Vec<(String, TypeId)>,
    pub functions: Vec<&'a Function>,
    pub interfaces: Vec<(String, &'a Interface)>,
}

#[derive(Debug, Clone)]
pub struct InterfaceStub {
    pub name: String,
    pub constructor_params: Option<Vec<FunctionParamStub>>,
    pub functions: Vec<FunctionStub>,
    pub static_functions: Vec<FunctionStub>,
    pub imports: Vec<InterfaceStubTypeDef>,
    pub global: bool,
    pub owner_interface: Option<String>,
}

impl InterfaceStub {
    pub fn is_resource(&self) -> bool {
        self.constructor_params.is_some()
    }

    pub fn interface_name(&self) -> Option<&str> {
        if self.global {
            None
        } else {
            match &self.owner_interface {
                Some(iface) => Some(iface),
                None => Some(&self.name),
            }
        }
    }

    pub fn resource_name(&self) -> Option<&str> {
        if self.is_resource() {
            Some(&self.name)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceStubTypeDef {
    pub name: String,
    pub path: String,
    pub package_name: Option<PackageName>,
    pub type_def: TypeDef,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct InterfaceStubImport {
    pub name: String,
    pub path: String,
}

impl From<&InterfaceStubTypeDef> for InterfaceStubImport {
    fn from(value: &InterfaceStubTypeDef) -> Self {
        Self {
            name: value.name.clone(),
            path: value.path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionStub {
    pub name: String,
    pub params: Vec<FunctionParamStub>,
    pub results: FunctionResultStub,
}

impl FunctionStub {
    pub fn as_method(&self) -> Option<FunctionStub> {
        self.name.strip_prefix("[method]").and_then(|method_name| {
            let parts = method_name.split('.').collect::<Vec<_>>();
            if parts.len() != 2 {
                None
            } else {
                Some(FunctionStub {
                    name: parts[1].to_string(),
                    params: self
                        .params
                        .iter()
                        .filter(|p| p.name != "self")
                        .cloned()
                        .collect(),
                    results: self.results.clone(),
                })
            }
        })
    }

    pub fn as_static(&self) -> Option<FunctionStub> {
        self.name.strip_prefix("[static]").and_then(|method_name| {
            let parts = method_name.split('.').collect::<Vec<_>>();
            if parts.len() != 2 {
                None
            } else {
                Some(FunctionStub {
                    name: parts[1].to_string(),
                    params: self
                        .params
                        .iter()
                        .filter(|p| p.name != "self")
                        .cloned()
                        .collect(),
                    results: self.results.clone(),
                })
            }
        })
    }

    pub fn async_result_type(&self, owner: &InterfaceStub) -> String {
        if owner.is_resource() {
            format!("future-{}-{}-result", owner.name, self.name)
        } else {
            format!("future-{}-result", self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionParamStub {
    pub name: String,
    pub typ: Type,
}

#[derive(Debug, Clone)]
pub enum FunctionResultStub {
    Single(Type),
    Multi(Vec<FunctionParamStub>),
    SelfType,
}

impl FunctionResultStub {
    pub fn is_empty(&self) -> bool {
        match self {
            FunctionResultStub::Single(_) => false,
            FunctionResultStub::Multi(params) => params.is_empty(),
            FunctionResultStub::SelfType => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StubTypeOwner {
    StubInterface,
    Source(TypeOwner),
}
