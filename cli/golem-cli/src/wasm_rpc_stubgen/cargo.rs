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

use crate::fs;
use crate::fs::PathExtra;
use crate::log::{log_action, log_warn_action, LogColorize};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::stub::StubDefinition;
use crate::wasm_rpc_stubgen::wit_resolve::ResolvedWitDir;
use crate::wasm_rpc_stubgen::{
    GOLEM_API_VERSION, GOLEM_RPC_WIT_VERSION, WASI_WIT_VERSION, WIT_BINDGEN_VERSION,
};
use anyhow::{anyhow, Context};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting, Workspace,
};
use heck::ToSnakeCase;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use toml::Value;
use toml_edit::{DocumentMut, InlineTable};
use wit_parser::PackageName;

#[derive(Serialize, Deserialize, Default)]
struct MetadataRoot {
    component: Option<ComponentMetadata>,
}

#[derive(Serialize, Deserialize)]
struct ComponentMetadata {
    package: Option<String>,
    target: Option<ComponentTarget>,
    bindings: Option<Bindings>,
}

#[derive(Serialize, Deserialize)]
struct ComponentTarget {
    world: Option<String>,
    #[serde(default = "default_path")]
    path: String,
    #[serde(default)]
    dependencies: BTreeMap<String, WitDependency>,
}

fn default_path() -> String {
    "wit".to_string()
}

impl Default for ComponentTarget {
    fn default() -> Self {
        Self {
            world: None,
            path: "wit".to_string(),
            dependencies: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct WitDependency {
    path: String,
}

#[derive(Serialize, Deserialize)]
struct Bindings {
    with: HashMap<String, String>,
}

pub fn generate_client_cargo_toml(def: &StubDefinition) -> anyhow::Result<()> {
    let mut manifest = Manifest::default();

    if def.config.seal_cargo_workspace {
        manifest.workspace = Some(Workspace::default());
    }

    let mut wit_dependencies = BTreeMap::new();

    wit_dependencies.insert(
        "golem:rpc".to_string(),
        WitDependency {
            path: "wit/deps/golem-rpc".to_string(),
        },
    );

    wit_dependencies.insert(
        "wasi:io".to_string(),
        WitDependency {
            path: "wit/deps/io".to_string(),
        },
    );

    wit_dependencies.insert(
        "wasi:clocks".to_string(),
        WitDependency {
            path: "wit/deps/clocks".to_string(),
        },
    );

    let stub_dep_package_ids = def.stub_dep_package_ids();
    for (dep_package_id, dep_package, dep_package_sources) in def.packages_with_wit_sources() {
        if !stub_dep_package_ids.contains(&dep_package_id) {
            continue;
        }

        if dep_package.name == def.source_package_name {
            wit_dependencies.insert(
                format_package_name_without_version(&def.source_package_name),
                WitDependency {
                    path: naming::wit::package_wit_dep_dir_from_parser(&def.source_package_name)
                        .to_string_lossy()
                        .to_string(),
                },
            );
        } else {
            wit_dependencies.insert(
                format_package_name_without_version(&dep_package.name),
                WitDependency {
                    path: naming::wit::package_wit_dep_dir_from_package_dir_name(
                        &PathExtra::new(&dep_package_sources.dir).file_name_to_string()?,
                    )
                    .to_string_lossy()
                    .to_string(),
                },
            );
        }
    }

    let bindings = {
        let mut with = HashMap::new();

        def.client_binding_mapping
            .add_to_cargo_bindings_table(&mut with);
        Bindings { with }
    };

    let metadata = MetadataRoot {
        component: Some(ComponentMetadata {
            package: Some(format_package_name_without_version(
                &def.source_package_name,
            )),
            target: Some(ComponentTarget {
                world: Some(def.client_world_name()),
                path: "wit".to_string(),
                dependencies: wit_dependencies,
            }),
            bindings: Some(bindings),
        }),
    };

    let mut package =
        cargo_toml::Package::new(def.client_crate_name(), &def.config.stub_crate_version);
    package.edition = Inheritable::Set(Edition::E2021);
    package.metadata = Some(metadata);
    manifest.package = Some(package);

    let lib = cargo_toml::Product {
        path: Some("src/lib.rs".to_string()),
        crate_type: vec!["cdylib".to_string()],
        ..Default::default()
    };
    manifest.lib = Some(lib);

    manifest.profile = Profiles {
        release: Some(Profile {
            lto: Some(LtoSetting::Fat),
            opt_level: Some(Value::String("s".to_string())),
            debug: None,
            split_debuginfo: None,
            rpath: None,
            debug_assertions: None,
            codegen_units: None,
            panic: None,
            incremental: None,
            overflow_checks: None,
            strip: Some(StripSetting::Symbols),
            package: BTreeMap::new(),
            build_override: None,
            inherits: None,
        }),
        ..Default::default()
    };

    let dep_wit_bindgen = Dependency::Detailed(Box::new(DependencyDetail {
        version: Some(WIT_BINDGEN_VERSION.to_string()),
        features: vec!["bitflags".to_string()],
        ..Default::default()
    }));

    let dep_golem_rust = Dependency::Detailed(Box::new(DependencyDetail {
        version: if def.config.golem_rust_override.path_override.is_none() {
            if let Some(version) = def.config.golem_rust_override.version_override.as_ref() {
                Some(version.to_string())
            } else {
                Some("1.7.0".to_string()) // TODO: constant
            }
        } else {
            None
        },
        path: def
            .config
            .golem_rust_override
            .path_override
            .as_ref()
            .map(|path| {
                path.to_str()
                    .expect("Failed to convert golem-rust override path to string")
                    .to_string()
            }),
        default_features: false,
        features: vec![],
        ..Default::default()
    }));

    let mut deps = DepsSet::new();
    deps.insert("wit-bindgen-rt".to_string(), dep_wit_bindgen);
    deps.insert("golem-rust".to_string(), dep_golem_rust);
    manifest.dependencies = deps;

    let cargo_toml = toml::to_string(&manifest)?;

    log_action(
        "Generating",
        format!(
            "Cargo.toml to {}",
            def.client_cargo_path().log_color_highlight()
        ),
    );
    fs::write(def.client_cargo_path(), cargo_toml)?;
    Ok(())
}

pub fn is_cargo_component_toml(path: &Path) -> anyhow::Result<bool> {
    let manifest: Manifest<MetadataRoot> = Manifest::from_path_with_metadata(path)?;

    if let Some(package) = manifest.package {
        if let Some(metadata) = package.metadata {
            if metadata.component.is_some() {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

pub fn is_cargo_workspace_toml(path: &Path) -> anyhow::Result<bool> {
    let manifest = Manifest::from_path(path)?;
    if let Some(workspace) = manifest.workspace {
        if !workspace.members.is_empty() {
            Ok(true)
        } else {
            Ok(false)
        }
    } else {
        Ok(false)
    }
}

pub fn add_cargo_package_component_deps(
    cargo_toml_path: &Path,
    wit_sources: BTreeMap<PackageName, PathBuf>,
) -> anyhow::Result<()> {
    let raw_manifest = fs::read_to_string(cargo_toml_path)?;
    let mut manifest: Manifest<MetadataRoot> =
        Manifest::from_slice_with_metadata(raw_manifest.as_bytes())?;
    if let Some(ref mut package) = manifest.package {
        if let Some(ref mut metadata) = package.metadata {
            if let Some(ref mut component) = metadata.component {
                let mut new_target = ComponentTarget::default();
                let target = component.target.as_mut().unwrap_or(&mut new_target);
                let existing: BTreeSet<_> = target.dependencies.keys().cloned().collect();
                for (package_name, package_path) in wit_sources {
                    let name = format_package_name_without_version(&package_name);
                    if !existing.contains(&name) {
                        target.dependencies.insert(
                            name,
                            WitDependency {
                                path: package_path.to_string_lossy().to_string(),
                            },
                        );
                    }
                }

                if component.target.is_none() {
                    component.target = Some(new_target);
                }

                let cargo_toml = toml::to_string(&manifest)?;

                log_warn_action(
                    "Updating",
                    format!("Cargo.toml at {}", cargo_toml_path.log_color_highlight()),
                );
                fs::write(cargo_toml_path, cargo_toml)?;
            }
        }
    }

    Ok(())
}

pub fn regenerate_cargo_package_component(
    cargo_toml_path: &Path,
    wit_path: &Path,
    world: Option<String>,
) -> anyhow::Result<()> {
    let cargo_toml_path = PathExtra::new(cargo_toml_path);

    log_warn_action(
        "Regenerating",
        format!(
            "package component in {}",
            cargo_toml_path.log_color_highlight()
        ),
    );

    let project_root = cargo_toml_path.parent()?;
    let relative_wit_path = wit_path.strip_prefix(project_root).with_context(|| {
        anyhow!(
            "Failed to create relative path for wit dir: {}, project root: {}",
            wit_path.log_color_highlight(),
            project_root.log_color_highlight()
        )
    })?;

    let raw_manifest = fs::read_to_string(&cargo_toml_path).with_context(|| {
        anyhow!(
            "Failed to read Cargo.toml at {}",
            cargo_toml_path.log_color_highlight()
        )
    })?;

    let mut manifest = raw_manifest.parse::<DocumentMut>().with_context(|| {
        anyhow!(
            "Failed to parse cargo project file: {}",
            cargo_toml_path.display()
        )
    })?;

    let has_golem_rust = if let Some(dependencies) = manifest.get("dependencies") {
        let deps_table = dependencies.as_table().ok_or_else(|| {
            anyhow!(
                "Expected table for dependencies in {}",
                cargo_toml_path.display()
            )
        })?;
        deps_table.contains_key("golem-rust")
    } else {
        false
    };

    let component = manifest["package"] //
        .or_insert(toml_edit::table())["metadata"]
        .or_insert(toml_edit::table())["component"]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| {
            anyhow!(
                "Expected table for package.metadata.component in {}",
                cargo_toml_path.display()
            )
        })?;

    let target = component["target"]
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| {
            anyhow!(
                "Expected table for package.metadata.component.target in {}",
                cargo_toml_path.display()
            )
        })?;
    target["path"] = toml_edit::value(relative_wit_path.to_string_lossy().to_string());
    match world {
        Some(world) => {
            target["world"] = toml_edit::value(world);
        }
        None => {
            target.remove("world");
        }
    }

    let dependencies = target
        .entry("dependencies")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .ok_or_else(|| {
            anyhow!(
                "Expected table for package.metadata.component.dependencies in {}",
                cargo_toml_path.display()
            )
        })?;

    dependencies.clear();

    let wit_packages_in_golem_rust: HashSet<&'static str> = HashSet::from_iter([
        "golem:api",
        "golem:durability",
        "golem:rdbms",
        "golem:rpc",
        "wasi:clocks",
        "wasi:io",
        "wasi:http",
        "wasi:random",
        "wasi:cli",
        "wasi:filesystem",
        "wasi:sockets",
        "wasi:blobstore",
        "wasi:keyvalue",
        "wasi:logging",
    ]);
    let mut bind_to_golem_rust = Vec::new();

    let wit_dir = ResolvedWitDir::new(wit_path)?;
    for (package_id, package_sources) in &wit_dir.package_sources {
        if *package_id == wit_dir.package_id {
            continue;
        }

        let dep_package = wit_dir.package(*package_id)?;
        let dep_package_name = &dep_package.name;
        let dep_name = format_package_name_without_version(dep_package_name);

        let used = wit_dir.used_interfaces()?;
        let used = used
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<HashSet<_>>();

        if has_golem_rust && wit_packages_in_golem_rust.contains(dep_name.as_str()) {
            for (interface_name, interface_id) in &dep_package.interfaces {
                if used.contains(interface_id) {
                    let interface_path = dep_package_name.interface_id(interface_name);

                    if interface_path == format!("golem:rpc/types@{GOLEM_RPC_WIT_VERSION}") {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::wasm_rpc::golem_rpc_0_2_x::types".to_string(),
                        ));
                    } else if interface_path == format!("wasi:io/poll@{WASI_WIT_VERSION}") {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::wasm_rpc::wasi::io::poll".to_string(),
                        ));
                    } else if interface_path == format!("wasi:clocks/wall-clock@{WASI_WIT_VERSION}")
                    {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::wasm_rpc::wasi::clocks::wall_clock".to_string(),
                        ));
                    } else if interface_path
                        == format!("golem:api/load-snapshot@{GOLEM_API_VERSION}")
                    {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::load_snapshot::exports::golem::api::load_snapshot"
                                .to_string(),
                        ));
                    } else if interface_path
                        == format!("golem:api/save-snapshot@{GOLEM_API_VERSION}")
                    {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::save_snapshot::exports::golem::api::save_snapshot"
                                .to_string(),
                        ));
                    } else if interface_path
                        == format!("golem:api/oplog-processor@{GOLEM_API_VERSION}")
                    {
                        bind_to_golem_rust.push((
                            interface_path,
                            "golem_rust::oplog_processor::exports::golem::api::oplog_processor"
                                .to_string(),
                        ));
                    } else {
                        bind_to_golem_rust.push((
                            interface_path,
                            format!(
                                "golem_rust::bindings::{}::{}::{}",
                                dep_package_name.namespace.to_snake_case(),
                                dep_package_name.name.to_snake_case(),
                                interface_name.to_snake_case()
                            ),
                        ))
                    }
                }
            }
        }

        let mut dep = InlineTable::new();
        dep.insert(
            "path",
            PathExtra::new(PathExtra::new(&package_sources.dir).strip_prefix(project_root)?)
                .to_string()?
                .into(),
        );

        dependencies[&dep_name] = toml_edit::value(dep);
    }

    if has_golem_rust && !bind_to_golem_rust.is_empty() {
        let with = component["bindings"].or_insert(toml_edit::table())["with"]
            .or_insert(toml_edit::table())
            .as_table_mut()
            .ok_or_else(|| {
                anyhow!(
                    "Expected table for package.metadata.component.bindings.with in {}",
                    cargo_toml_path.display()
                )
            })?;

        for (from, to) in bind_to_golem_rust {
            with[&from] = toml_edit::value(to);
        }
    }

    fs::write(cargo_toml_path, manifest.to_string())?;

    Ok(())
}

fn format_package_name_without_version(package_name: &PackageName) -> String {
    format!("{}:{}", package_name.namespace, package_name.name)
}
