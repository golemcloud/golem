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

use crate::fs::PathExtra;
use crate::log::{log_action, log_warn_action, LogColorize};
use crate::stub::StubDefinition;
use crate::wit_resolve::ResolvedWitDir;
use crate::{fs, naming};
use anyhow::{anyhow, Context};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting, Workspace,
};
use golem_wasm_rpc::WASM_RPC_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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

    if def.config.seal_cargo_workspace {
        manifest.workspace = Some(Workspace::default());
    }

    let mut wit_dependencies = BTreeMap::new();

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
        version: Some("0.26.0".to_string()),
        features: vec!["bitflags".to_string()],
        ..Default::default()
    }));

    let dep_golem_wasm_rpc = Dependency::Detailed(Box::new(DependencyDetail {
        version: if def
            .config
            .wasm_rpc_override
            .wasm_rpc_path_override
            .is_none()
        {
            if let Some(version) = def
                .config
                .wasm_rpc_override
                .wasm_rpc_version_override
                .as_ref()
            {
                Some(version.to_string())
            } else {
                Some(WASM_RPC_VERSION.to_string())
            }
        } else {
            None
        },
        path: def.config.wasm_rpc_override.wasm_rpc_path_override.clone(),
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

    log_action(
        "Writing",
        format!("updated Cargo.toml to {:?}", path.log_color_highlight()),
    );
    fs::write(path, cargo_toml)?;
    Ok(())
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
    let mut manifest: Manifest<MetadataRoot> =
        Manifest::from_slice_with_metadata(raw_manifest.as_bytes()).with_context(|| {
            anyhow!(
                "Failed to parse Cargo.toml at {}",
                cargo_toml_path.log_color_highlight()
            )
        })?;
    let package = manifest.package.as_mut().ok_or_else(|| {
        anyhow!(
            "No package found in {}",
            cargo_toml_path.log_color_highlight()
        )
    })?;

    let wit_dir = ResolvedWitDir::new(wit_path)?;

    package.metadata = Some(MetadataRoot {
        component: Some(ComponentMetadata {
            package: None,
            target: Some(ComponentTarget {
                world,
                path: relative_wit_path.to_string_lossy().to_string(),
                dependencies: wit_dir
                    .package_sources
                    .iter()
                    .filter(|(&package_id, _)| package_id != wit_dir.package_id)
                    .map(|(package_id, package_sources)| {
                        (
                            format_package_name_without_version(
                                &wit_dir.package(*package_id).unwrap().name,
                            ),
                            WitDependency {
                                path: PathExtra::new(
                                    PathExtra::new(&package_sources.dir)
                                        .strip_prefix(project_root)
                                        .unwrap(),
                                )
                                .to_string()
                                .unwrap(),
                            },
                        )
                    })
                    .collect(),
            }),
        }),
    });

    let cargo_toml = toml::to_string(&manifest)?;
    fs::write(cargo_toml_path, cargo_toml)?;

    Ok(())
}

fn format_package_name_without_version(package_name: &PackageName) -> String {
    format!("{}:{}", package_name.namespace, package_name.name)
}
