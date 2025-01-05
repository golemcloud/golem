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

use crate::wit_encode::EncodedWitDir;
use crate::wit_generate::extract_main_interface_as_wit_dep;
use crate::wit_resolve::{PackageSource, ResolvedWitDir};
use crate::{naming, WasmRpcOverride};
use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use itertools::Itertools;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use wit_parser::{
    Function, FunctionKind, Interface, InterfaceId, Package, PackageId, PackageName, Resolve,
    Results, Type, TypeDef, TypeDefKind, TypeId, TypeOwner, World, WorldId, WorldItem, WorldKey,
};

#[derive(Clone, Debug)]
pub struct StubConfig {
    pub source_wit_root: PathBuf,
    pub target_root: PathBuf,
    pub selected_world: Option<String>,
    pub stub_crate_version: String,
    pub wasm_rpc_override: WasmRpcOverride,
    pub extract_source_interface_package: bool,
    pub seal_cargo_workspace: bool,
}

pub struct StubDefinition {
    pub config: StubConfig,

    resolve: Resolve,
    source_world_id: WorldId,
    package_sources: IndexMap<PackageId, PackageSource>,

    stub_imported_interfaces: OnceCell<Vec<InterfaceStub>>,
    stub_used_type_defs: OnceCell<Vec<InterfaceStubTypeDef>>,
    stub_dep_package_ids: OnceCell<HashSet<PackageId>>,

    pub source_package_id: PackageId,
    pub source_package_name: PackageName,
}

impl StubDefinition {
    pub fn new(config: StubConfig) -> anyhow::Result<Self> {
        if config.extract_source_interface_package {
            extract_main_interface_as_wit_dep(&config.source_wit_root)
                .context("Failed to extract the source interface package")?
        }

        let resolved_source = ResolvedWitDir::new(&config.source_wit_root)?;

        let source_world_id = resolved_source
            .resolve
            .select_world(resolved_source.package_id, config.selected_world.as_deref())?;

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
            config,
            resolve: resolved_source.resolve,
            source_world_id,
            package_sources: resolved_source.package_sources,
            stub_imported_interfaces: OnceCell::new(),
            stub_used_type_defs: OnceCell::new(),
            stub_dep_package_ids: OnceCell::new(),
            source_package_id: resolved_source.package_id,
            source_package_name,
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
    ) -> impl Iterator<Item = (PackageId, &Package, &PackageSource)> {
        self.resolve
            .topological_packages()
            .into_iter()
            .map(|package_id| {
                (
                    package_id,
                    self.resolve
                        .packages
                        .get(package_id)
                        .unwrap_or_else(|| panic!("package not found")),
                    self.package_sources
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

    pub fn source_world_name(&self) -> &str {
        &self.source_world().name
    }

    pub fn encode_source(&self) -> anyhow::Result<EncodedWitDir> {
        EncodedWitDir::new(&self.resolve)
    }

    pub fn target_cargo_path(&self) -> PathBuf {
        self.config.target_root.join("Cargo.toml")
    }

    pub fn target_crate_name(&self) -> String {
        format!("{}-stub", self.source_world_name())
    }

    pub fn target_rust_path(&self) -> PathBuf {
        self.config.target_root.join("src/lib.rs")
    }

    pub fn target_interface_name(&self) -> String {
        // TODO: naming
        format!("stub-{}", self.source_world_name())
    }

    pub fn target_world_name(&self) -> String {
        // TODO: naming
        format!("wasm-rpc-stub-{}", self.source_world_name())
    }

    pub fn target_wit_root(&self) -> PathBuf {
        self.config.target_root.join(naming::wit::WIT_DIR)
    }

    pub fn target_wit_path(&self) -> PathBuf {
        self.target_wit_root().join(naming::wit::STUB_WIT_FILE_NAME)
    }

    pub fn resolve_target_wit(&self) -> anyhow::Result<ResolvedWitDir> {
        ResolvedWitDir::new(&self.target_wit_root())
    }

    pub fn stub_imported_interfaces(&self) -> &Vec<InterfaceStub> {
        self.stub_imported_interfaces.get_or_init(|| {
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

    pub fn stub_used_type_defs(&self) -> &Vec<InterfaceStubTypeDef> {
        self.stub_used_type_defs.get_or_init(|| {
            let imported_type_ids = self
                .stub_imported_interfaces()
                .iter()
                .flat_map(|interface| &interface.used_types)
                .cloned()
                .collect::<HashSet<_>>();

            let imported_type_names = {
                let mut imported_type_names = HashMap::<String, HashSet<TypeId>>::new();
                for type_id in imported_type_ids {
                    let type_def = self.resolve.types.get(type_id).unwrap_or_else(|| {
                        panic!("Imported type not found, type id: {:?}", type_id)
                    });
                    let type_name = type_def
                        .name
                        .clone()
                        .unwrap_or_else(|| panic!("Missing type name, type id: {:?}", type_id));

                    imported_type_names
                        .entry(type_name)
                        .or_default()
                        .insert(type_id);
                }
                imported_type_names
            };

            imported_type_names
                .into_iter()
                .flat_map(|(type_name, type_ids)| {
                    let type_name_is_unique = type_ids.len() == 1;
                    type_ids.into_iter().map(move |type_id| {
                        let type_def = self
                            .resolve
                            .types
                            .get(type_id)
                            .unwrap_or_else(|| {
                                panic!("Imported type not found, type id: {:?}", type_id)
                            })
                            .clone();

                        let TypeOwner::Interface(interface_id) = type_def.owner else {
                            panic!(
                                "Expected interface owner for type, got: {:?}, type name: {:?}",
                                type_def, type_def.name
                            );
                        };

                        let interface =
                            self.resolve
                                .interfaces
                                .get(interface_id)
                                .unwrap_or_else(|| {
                                    panic!("Interface not found, interface id: {:?}", interface_id)
                                });

                        let interface_name = interface.name.clone().unwrap_or_else(|| {
                            panic!("Missing interface name, interface id: {:?}", interface_id)
                        });

                        let package = interface.package.map(|package_id| {
                            self.resolve.packages.get(package_id).unwrap_or_else(|| {
                                panic!(
                                    "Missing package for interface, package id: {:?}, interface id: {:?}",
                                    package_id, interface_id,
                                )
                            })
                        });

                        let interface_identifier = package
                            .map(|package| package.name.interface_id(&interface_name))
                            .unwrap_or(interface_name.clone());

                        let type_name_alias = (!type_name_is_unique).then(|| match &package {
                            Some(package) => format!(
                                "{}-{}-{}-{}",
                                package.name.namespace,
                                package.name.name,
                                interface_name,
                                &type_name
                            ),
                            None => format!("{}-{}", interface_name, type_name),
                        });

                        InterfaceStubTypeDef {
                            package_id: interface.package,
                            type_id,
                            type_def,
                            interface_identifier,
                            type_name: type_name.clone(),
                            type_name_alias,
                        }
                    })
                })
                .sorted_by(|a, b| {
                    let a = (&a.interface_identifier, &a.type_name, &a.type_name_alias);
                    let b = (&b.interface_identifier, &b.type_name, &b.type_name_alias);
                    a.cmp(&b)
                })
                .collect::<Vec<_>>()
        })
    }

    pub fn stub_dep_package_ids(&self) -> &HashSet<PackageId> {
        self.stub_dep_package_ids.get_or_init(|| {
            self.stub_used_type_defs()
                .iter()
                .flat_map(|type_def| {
                    let mut package_ids = Vec::<PackageId>::new();
                    self.type_def_owner_package_ids(&type_def.type_def, &mut package_ids);
                    package_ids
                })
                .collect()
        })
    }

    fn type_def_owner_package_ids(&self, type_def: &TypeDef, package_ids: &mut Vec<PackageId>) {
        let package_id = match type_def.owner {
            TypeOwner::World(_) => None,
            TypeOwner::Interface(interface) => self
                .resolve
                .interfaces
                .get(interface)
                .and_then(|interface| interface.package),
            TypeOwner::None => None,
        };

        if let Some(package_id) = package_id {
            package_ids.push(package_id);
        }

        if let TypeDefKind::Type(Type::Id(type_id)) = type_def.kind {
            self.type_def_owner_package_ids(
                self.resolve.types.get(type_id).unwrap_or_else(|| {
                    panic!("Type alias target not found, type id: {:?}", type_id)
                }),
                package_ids,
            );
        }
    }

    fn partition_world_items<'a>(
        &'a self,
        world_items: &'a IndexMap<WorldKey, WorldItem>,
    ) -> WorldItemsByType<'a> {
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
                FunctionResultStub::Named(param_stubs)
            }
            Results::Anon(single) => FunctionResultStub::Anon(*single),
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

        let (used_types, _) = self.extract_interface_stubs_from_types(types.into_iter());

        Some(InterfaceStub {
            name: name.to_string(),
            functions,
            used_types,
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
        let package = interface
            .package
            .map(|package_id| self.resolve.packages.get(package_id).unwrap());
        let owner_interface = interface.name.as_ref().map(|name| match package {
            Some(package) => package.name.interface_id(name),
            None => name.to_string(),
        });

        let functions = Self::functions_to_stubs(interface.functions.values());

        let (used_types, resource_interfaces) = self.extract_interface_stubs_from_types(
            interface
                .types
                .iter()
                .map(|(name, typ)| (name.clone(), *typ)),
        );

        let mut interface_stubs = Vec::with_capacity(1 + resource_interfaces.len());

        interface_stubs.push(InterfaceStub {
            name,
            functions,
            used_types,
            global: false,
            constructor_params: None,
            static_functions: vec![],
            owner_interface,
        });
        interface_stubs.extend(resource_interfaces);

        interface_stubs
    }

    fn extract_interface_stubs_from_types(
        &self,
        types: impl Iterator<Item = (String, TypeId)>,
    ) -> (Vec<TypeId>, Vec<InterfaceStub>) {
        let mut used_types = Vec::<TypeId>::new();
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
                    used_types.push(type_id);
                }
            }
        }

        (used_types, resource_interfaces)
    }

    fn resource_interface_stub(
        &self,
        owner_interface: &Interface,
        type_name: String,
        type_id: TypeId,
    ) -> InterfaceStub {
        let package = owner_interface
            .package
            .map(|package_id| self.resolve.packages.get(package_id).unwrap());
        let owner_interface_name = owner_interface.name.as_ref().map(|name| match package {
            Some(package) => package.name.interface_id(name),
            None => name.to_string(),
        });

        let functions_by_kind = |kind: FunctionKind| {
            owner_interface
                .functions
                .values()
                .filter(move |function| function.kind == kind)
        };

        let function_stubs_by_kind =
            |kind: FunctionKind| Self::functions_to_stubs(functions_by_kind(kind));

        let (used_types, _) = self.extract_interface_stubs_from_types(
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
            used_types,
            global: false,
            constructor_params,
            static_functions: function_stubs_by_kind(FunctionKind::Static(type_id)),
            owner_interface: owner_interface_name,
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

    pub fn get_stub_used_type_alias(&self, type_id: TypeId) -> Option<&str> {
        // TODO: hash map
        self.stub_used_type_defs()
            .iter()
            .find(|type_def| type_def.type_id == type_id)
            .and_then(|type_def| type_def.type_name_alias.as_deref())
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
    pub used_types: Vec<TypeId>,
    pub global: bool,
    pub owner_interface: Option<String>,
}

impl InterfaceStub {
    // The returned bool is true if the function is static
    pub fn all_functions(&self) -> impl Iterator<Item = (&FunctionStub, bool)> {
        self.static_functions
            .iter()
            .map(|f| (f, true))
            .chain(self.functions.iter().map(|f| (f, false)))
    }
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
    pub package_id: Option<PackageId>,
    pub type_id: TypeId,
    pub type_def: TypeDef,
    pub interface_identifier: String,
    pub type_name: String,
    pub type_name_alias: Option<String>,
}

impl InterfaceStubTypeDef {
    pub fn stub_type_name(&self) -> &str {
        self.type_name_alias.as_deref().unwrap_or(&self.type_name)
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
        self.as_function_stub_without_prefix("[method]")
    }

    pub fn as_static(&self) -> Option<FunctionStub> {
        self.as_function_stub_without_prefix("[static]")
    }

    fn as_function_stub_without_prefix(&self, prefix: &str) -> Option<FunctionStub> {
        self.name.strip_prefix(prefix).and_then(|method_name| {
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
    Anon(Type),
    Named(Vec<FunctionParamStub>),
    SelfType,
}

impl FunctionResultStub {
    pub fn is_empty(&self) -> bool {
        match self {
            FunctionResultStub::Anon(_) => false,
            FunctionResultStub::Named(params) => params.is_empty(),
            FunctionResultStub::SelfType => false,
        }
    }
}
