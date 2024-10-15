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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::log::{log_action, log_warn_action};
use crate::stub::StubDefinition;
use crate::wit;
use anyhow::{anyhow, bail, Context};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting,
};
use golem_wasm_rpc::WASM_RPC_VERSION;
use serde::{Deserialize, Serialize};
use toml::Value;
use wit_parser::PackageName;

#[derive(Serialize, Deserialize, Default)]
struct MetadataRoot {
    component: Option<ComponentMetadata>,
}

#[derive(Serialize, Deserialize)]
struct ComponentMetadata {
    package: Option<String>,
    target: Option<ComponentTarget>,
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

pub fn generate_cargo_toml(def: &StubDefinition) -> anyhow::Result<()> {
    let mut manifest = Manifest::default();

    let mut wit_dependencies = BTreeMap::new();

    wit_dependencies.insert(
        def.source_package_name.name.to_string(),
        WitDependency {
            path: format!(
                "wit/deps/{}_{}",
                def.source_package_name.namespace, def.source_package_name.name
            ),
        },
    );
    wit_dependencies.insert(
        "golem:rpc".to_string(),
        WitDependency {
            path: "wit/deps/wasm-rpc".to_string(),
        },
    );

    wit_dependencies.insert(
        "wasi:io".to_string(),
        WitDependency {
            path: "wit/deps/io".to_string(),
        },
    );

    let stub_package_name = def.stub_package_name();
    for (dep_package, dep_package_sources) in def.source_packages_with_sources() {
        let dep_package_name = &dep_package.name;

        if dep_package_name.to_string() == stub_package_name {
            log_warn_action(
                "Skipping",
                format!("updating WIT dependency for {}", dep_package_name),
            );
            continue;
        }

        let mut dirs = BTreeSet::new();
        for source in dep_package_sources {
            let relative = source.strip_prefix(&def.source_wit_root)?;
            let dir = relative
                .parent()
                .ok_or(anyhow!("Package source {source:?} has no parent directory"))?;
            dirs.insert(dir);
        }

        if dirs.len() != 1 {
            bail!(
                "Package {} has multiple source directories",
                dep_package_name
            );
        }

        wit_dependencies.insert(
            format_package_name_without_version(&dep_package.name),
            WitDependency {
                path: format!("wit/{}", dirs.iter().next().unwrap().to_str().unwrap()),
            },
        );
    }

    let metadata = MetadataRoot {
        component: Some(ComponentMetadata {
            package: Some(format_package_name_without_version(
                &def.source_package_name,
            )),
            target: Some(ComponentTarget {
                world: Some(def.target_world_name()),
                path: "wit".to_string(),
                dependencies: wit_dependencies,
            }),
        }),
    };

    let mut package = cargo_toml::Package::new(def.target_crate_name(), &def.stub_crate_version);
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
        version: Some("0.26.0".to_string()),
        features: vec!["bitflags".to_string()],
        ..Default::default()
    }));

    let dep_golem_wasm_rpc = Dependency::Detailed(Box::new(DependencyDetail {
        version: if def.wasm_rpc_override.wasm_rpc_path_override.is_none() {
            if let Some(version) = def.wasm_rpc_override.wasm_rpc_version_override.as_ref() {
                Some(version.to_string())
            } else {
                Some(WASM_RPC_VERSION.to_string())
            }
        } else {
            None
        },
        path: def.wasm_rpc_override.wasm_rpc_path_override.clone(),
        default_features: false,
        features: vec!["stub".to_string()],
        ..Default::default()
    }));

    let mut deps = DepsSet::new();
    deps.insert("wit-bindgen-rt".to_string(), dep_wit_bindgen);
    deps.insert("golem-wasm-rpc".to_string(), dep_golem_wasm_rpc);
    manifest.dependencies = deps;

    let cargo_toml = toml::to_string(&manifest)?;

    log_action(
        "Generating",
        format!(
            "Cargo.toml to {}",
            def.target_cargo_path().to_string_lossy()
        ),
    );
    fs::write(def.target_cargo_path(), cargo_toml)?;
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

pub fn add_workspace_members(path: &Path, members: &[String]) -> anyhow::Result<()> {
    let mut manifest = Manifest::from_path(path)?;
    if let Some(workspace) = manifest.workspace.as_mut() {
        for member in members {
            if !workspace.members.contains(member) {
                workspace.members.push(member.to_string());
            }
        }
    }

    let cargo_toml = toml::to_string(&manifest)?;

    log_action("Writing", format!("updated Cargo.toml to {:?}", path));
    fs::write(path, cargo_toml)?;
    Ok(())
}

pub fn add_dependencies_to_cargo_toml(
    cargo_path: &Path,
    wit_sources: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let raw_manifest = std::fs::read_to_string(cargo_path)?;
    let mut manifest: Manifest<MetadataRoot> =
        Manifest::from_slice_with_metadata(raw_manifest.as_bytes())?;
    if let Some(ref mut package) = manifest.package {
        if let Some(ref mut metadata) = package.metadata {
            if let Some(ref mut component) = metadata.component {
                let mut new_target = ComponentTarget::default();
                let target = component.target.as_mut().unwrap_or(&mut new_target);
                let existing: BTreeSet<_> = target.dependencies.keys().cloned().collect();
                /*for wit_source in wit_sources {
                    if !existing.contains(name) {
                        let relative_path = format!("wit/deps/{}", name);
                        let path = cargo_path
                            .parent()
                            .context("Parent directory of Cargo.toml")?
                            .join(&relative_path);
                        let package_name = wit::get_package_name(&path)?;

                        target.dependencies.insert(
                            format_package_name_without_version(&package_name),
                            WitDependency {
                                path: relative_path,
                            },
                        );
                    }
                }*/
                // TODO

                if component.target.is_none() {
                    component.target = Some(new_target);
                }

                let cargo_toml = toml::to_string(&manifest)?;

                log_warn_action("Updating", format!("Cargo.toml at {:?}", cargo_path));
                fs::write(cargo_path, cargo_toml)?;
            }
        }
    }

    Ok(())
}

fn format_package_name_without_version(package_name: &PackageName) -> String {
    format!("{}:{}", package_name.namespace, package_name.name)
}
