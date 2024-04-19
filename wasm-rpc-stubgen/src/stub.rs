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

use anyhow::{anyhow, bail};
use indexmap::IndexSet;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use wit_parser::{
    Function, FunctionKind, PackageName, Resolve, Results, Type, TypeDefKind, TypeId, TypeOwner,
    UnresolvedPackage, World, WorldId, WorldItem,
};

/// All the gathered information for generating the stub crate.
pub struct StubDefinition {
    pub resolve: Resolve,
    pub root_package_name: PackageName,
    pub world_id: WorldId,
    pub source_wit_root: PathBuf,
    pub target_root: PathBuf,
    pub stub_crate_version: String,
    pub interfaces: Vec<InterfaceStub>,
    pub unresolved_root: UnresolvedPackage,
    pub unresolved_deps: Vec<UnresolvedPackage>,
    pub wasm_rpc_path_override: Option<String>,
}

impl StubDefinition {
    pub fn new(
        source_wit_root: &Path,
        target_root: &Path,
        selected_world: &Option<String>,
        stub_crate_version: &str,
        wasm_rpc_path_override: &Option<String>,
    ) -> anyhow::Result<Self> {
        let (root, deps) = get_unresolved_packages(source_wit_root)?;
        let root_package = root.name.clone();

        let mut resolve = Resolve::new();
        for unresolved in deps.iter().cloned() {
            resolve.push(unresolved)?;
        }
        let root_id = resolve.push(root.clone())?;

        let world_id = resolve.select_world(root_id, selected_world.as_deref())?;
        let world = resolve
            .worlds
            .get(world_id)
            .ok_or(anyhow!("world {world_id:?} not found"))?;
        let interfaces = collect_stub_interfaces(&resolve, world)?;

        Ok(Self {
            resolve,
            root_package_name: root_package,
            world_id,
            source_wit_root: source_wit_root.to_path_buf(),
            target_root: target_root.to_path_buf(),
            stub_crate_version: stub_crate_version.to_string(),
            interfaces,
            unresolved_root: root,
            unresolved_deps: deps,
            wasm_rpc_path_override: wasm_rpc_path_override.clone(),
        })
    }

    pub fn source_world(&self) -> anyhow::Result<&World> {
        self.resolve
            .worlds
            .get(self.world_id)
            .ok_or(anyhow!("selected world not found"))
    }

    pub fn source_world_name(&self) -> anyhow::Result<String> {
        Ok(self.source_world()?.name.clone())
    }

    pub fn target_cargo_path(&self) -> PathBuf {
        self.target_root.join("Cargo.toml")
    }

    pub fn target_crate_name(&self) -> anyhow::Result<String> {
        Ok(format!("{}-stub", self.source_world_name()?))
    }

    pub fn target_rust_path(&self) -> PathBuf {
        self.target_root.join("src/lib.rs")
    }

    pub fn target_world_name(&self) -> anyhow::Result<String> {
        Ok(format!("wasm-rpc-stub-{}", self.source_world_name()?))
    }

    pub fn target_wit_root(&self) -> PathBuf {
        self.target_root.join("wit")
    }

    pub fn target_wit_path(&self) -> PathBuf {
        self.target_wit_root().join("_stub.wit")
    }

    pub fn verify_target_wits(&self) -> anyhow::Result<()> {
        let (final_root, final_deps) = get_unresolved_packages(&self.target_wit_root())?;

        let mut final_resolve = Resolve::new();
        for unresolved in final_deps.iter().cloned() {
            final_resolve.push(unresolved)?;
        }
        final_resolve.push(final_root.clone())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InterfaceStub {
    pub name: String,
    pub constructor_params: Option<Vec<FunctionParamStub>>,
    pub functions: Vec<FunctionStub>,
    pub static_functions: Vec<FunctionStub>,
    pub imports: Vec<InterfaceStubImport>,
    pub global: bool,
    pub owner_interface: Option<String>,
}

impl InterfaceStub {
    pub fn is_resource(&self) -> bool {
        self.constructor_params.is_some()
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct InterfaceStubImport {
    pub name: String,
    pub path: String,
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

fn collect_stub_imports<'a>(
    types: impl Iterator<Item = (&'a String, &'a TypeId)>,
    resolve: &Resolve,
) -> anyhow::Result<Vec<InterfaceStubImport>> {
    let mut imports = Vec::new();

    for (name, typ) in types {
        let typ = resolve
            .types
            .get(*typ)
            .ok_or(anyhow!("type {typ:?} not found"))?;
        if typ.kind != TypeDefKind::Resource {
            // We will redefine resources so no need to import them
            match typ.owner {
                TypeOwner::World(world_id) => {
                    let _world = resolve
                        .worlds
                        .get(world_id)
                        .ok_or(anyhow!("world {world_id:?} not found"))?;
                }
                TypeOwner::Interface(interface_id) => {
                    let interface = resolve
                        .interfaces
                        .get(interface_id)
                        .ok_or(anyhow!("interface {interface_id:?} not found"))?;
                    let package = interface.package.and_then(|id| resolve.packages.get(id));
                    let interface_name = interface.name.clone().unwrap_or("unknown".to_string());
                    let interface_path = package
                        .map(|p| p.name.interface_id(&interface_name))
                        .unwrap_or(interface_name);
                    imports.push(InterfaceStubImport {
                        name: name.clone(),
                        path: interface_path,
                    });
                }
                TypeOwner::None => {}
            }
        }
    }

    Ok(imports)
}

fn collect_stub_interfaces(resolve: &Resolve, world: &World) -> anyhow::Result<Vec<InterfaceStub>> {
    let top_level_types = world
        .exports
        .iter()
        .filter_map(|(name, item)| match item {
            WorldItem::Type(t) => Some((name.clone().unwrap_name(), t)),
            _ => None,
        })
        .collect::<Vec<_>>();

    let top_level_functions = world
        .exports
        .iter()
        .filter_map(|(_, item)| match item {
            WorldItem::Function(f) => Some(f),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut interfaces = Vec::new();
    for (name, item) in &world.exports {
        if let WorldItem::Interface(id) = item {
            let interface = resolve
                .interfaces
                .get(*id)
                .ok_or(anyhow!("exported interface not found"))?;
            let name = interface.name.clone().unwrap_or(String::from(name.clone()));
            let functions = collect_stub_functions(
                interface
                    .functions
                    .values()
                    .filter(|f| f.kind == FunctionKind::Freestanding),
            )?;
            let imports = collect_stub_imports(interface.types.iter(), resolve)?;
            let resource_interfaces =
                collect_stub_resources(&name, interface.types.iter(), resolve)?;

            interfaces.push(InterfaceStub {
                name,
                functions,
                imports,
                global: false,
                constructor_params: None,
                static_functions: vec![],
                owner_interface: None,
            });

            interfaces.extend(resource_interfaces);
        }
    }

    if !top_level_functions.is_empty() {
        interfaces.push(InterfaceStub {
            name: world.name.clone(),
            functions: collect_stub_functions(
                top_level_functions
                    .into_iter()
                    .filter(|f| f.kind == FunctionKind::Freestanding),
            )?,
            imports: collect_stub_imports(top_level_types.iter().map(|(k, v)| (k, *v)), resolve)?,
            global: true,
            constructor_params: None,
            static_functions: vec![],
            owner_interface: None,
        });
    }

    Ok(interfaces)
}

fn collect_stub_functions<'a>(
    functions: impl Iterator<Item = &'a Function>,
) -> anyhow::Result<Vec<FunctionStub>> {
    let mut function_stub = vec![];

    for f in functions {
        let mut params = Vec::new();
        for (name, typ) in &f.params {
            params.push(FunctionParamStub {
                name: name.clone(),
                typ: *typ,
            });
        }

        let results = match &f.results {
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

        if results.is_empty() {
            function_stub.push(FunctionStub {
                name: format!("blocking-{}", f.name),
                params: params.clone(),
                results: results.clone(),
            })
        }

        function_stub.push(FunctionStub {
            name: f.name.clone(),
            params,
            results,
        });
    }

    Ok(function_stub)
}

fn collect_stub_resources<'a>(
    owner_interface: &str,
    types: impl Iterator<Item = (&'a String, &'a TypeId)>,
    resolve: &'a Resolve,
) -> anyhow::Result<Vec<InterfaceStub>> {
    let mut interfaces = Vec::new();
    for (_name, type_id) in types {
        let typ = resolve
            .types
            .get(*type_id)
            .ok_or(anyhow!("type {type_id:?} not found"))?;
        if typ.kind == TypeDefKind::Resource {
            match typ.owner {
                TypeOwner::World(_) => {}
                TypeOwner::Interface(iface_id) => {
                    let iface = resolve
                        .interfaces
                        .get(iface_id)
                        .ok_or(anyhow!("interface {iface_id:?} not found"))?;

                    let constructors = iface
                        .functions
                        .values()
                        .filter(|f| f.kind == FunctionKind::Constructor(*type_id))
                        .collect::<Vec<_>>();
                    let methods = iface
                        .functions
                        .values()
                        .filter(|f| f.kind == FunctionKind::Method(*type_id))
                        .collect::<Vec<_>>();
                    let statics = iface
                        .functions
                        .values()
                        .filter(|f| f.kind == FunctionKind::Static(*type_id))
                        .collect::<Vec<_>>();

                    let functions = collect_stub_functions(methods.into_iter())?
                        .into_iter()
                        .map(|f| {
                            f.as_method()
                                .ok_or(anyhow!("Non-method function found among resource methods"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let static_functions = collect_stub_functions(statics.into_iter())?
                        .into_iter()
                        .map(|f| {
                            f.as_static()
                                .ok_or(anyhow!("Non-static function found among resource statics"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    let imports = collect_stub_imports(iface.types.iter(), resolve)?;

                    let constructor_params = constructors.first().map(|c| {
                        c.params
                            .iter()
                            .map(|(n, t)| FunctionParamStub {
                                name: n.clone(),
                                typ: *t,
                            })
                            .collect::<Vec<_>>()
                    });

                    let resource_name = typ
                        .name
                        .as_ref()
                        .ok_or(anyhow!("Resource type has no name"))?
                        .clone();

                    interfaces.push(InterfaceStub {
                        name: resource_name,
                        functions,
                        imports,
                        global: false,
                        constructor_params,
                        static_functions,
                        owner_interface: Some(owner_interface.to_string()),
                    });
                }
                TypeOwner::None => {}
            }
        }
    }
    Ok(interfaces)
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md

fn visit<'a>(
    pkg: &'a UnresolvedPackage,
    deps: &'a BTreeMap<PackageName, UnresolvedPackage>,
    order: &mut IndexSet<PackageName>,
    visiting: &mut HashSet<&'a PackageName>,
) -> anyhow::Result<()> {
    if order.contains(&pkg.name) {
        return Ok(());
    }
    for (dep, _) in pkg.foreign_deps.iter() {
        if !visiting.insert(dep) {
            bail!("package depends on itself");
        }
        let dep = deps
            .get(dep)
            .ok_or_else(|| anyhow!("failed to find package `{dep}` in `deps` directory"))?;
        visit(dep, deps, order, visiting)?;
        assert!(visiting.remove(&dep.name));
    }
    assert!(order.insert(pkg.name.clone()));
    Ok(())
}

// Copied and modified from `wit-parser` crate
fn get_unresolved_packages(
    root_path: &Path,
) -> anyhow::Result<(UnresolvedPackage, Vec<UnresolvedPackage>)> {
    let root = UnresolvedPackage::parse_dir(root_path)?;

    let mut deps = BTreeMap::new();
    let deps_path = root_path.join(Path::new("deps"));
    if deps_path.exists() {
        for dep_entry in fs::read_dir(deps_path)? {
            let dep_entry = dep_entry?;
            let dep = UnresolvedPackage::parse_path(&dep_entry.path())?;
            deps.insert(dep.name.clone(), dep);
        }
    }

    // Perform a simple topological sort which will bail out on cycles
    // and otherwise determine the order that packages must be added to
    // this `Resolve`.
    let mut order = IndexSet::new();
    let mut visiting = HashSet::new();
    for pkg in deps.values().chain([&root]) {
        visit(pkg, &deps, &mut order, &mut visiting)?;
    }

    let mut ordered_deps = Vec::new();
    for name in order {
        if let Some(pkg) = deps.remove(&name) {
            ordered_deps.push(pkg);
        }
    }

    Ok((root, ordered_deps))
}
