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

use crate::commands::log::log_warn_action;
use crate::stub::StubDefinition;
use crate::wit::{get_stub_wit, verify_action, StubTypeGen, WitAction};
use crate::{cargo, wit, WasmRpcOverride};
use anyhow::{anyhow, Context};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use wit_parser::{PackageName, UnresolvedPackage, UnresolvedPackageGroup};

#[derive(PartialEq, Eq)]
pub enum UpdateCargoToml {
    Update,
    UpdateIfExists,
    NoUpdate,
}

pub fn add_stub_dependency(
    stub_wit_root: &Path,
    dest_wit_root: &Path,
    overwrite: bool,
    update_cargo_toml: UpdateCargoToml,
) -> anyhow::Result<()> {
    // Parsing the destination WIT root
    let parsed_dest = UnresolvedPackageGroup::parse_dir(dest_wit_root)?;

    // Dependencies of stub as directories
    let stub_deps = wit::get_dep_dirs(stub_wit_root)?;

    let stub_wit = stub_wit_root.join("_stub.wit");
    let parsed_stub = UnresolvedPackageGroup::parse_file(&stub_wit)?;

    let destination_package_name = parsed_dest.main.name.clone();

    let stub_target_package_name = PackageName {
        name: parsed_stub
            .main
            .name
            .name
            .strip_suffix("-stub")
            .expect("Unexpected stub package name")
            .to_string(),
        ..parsed_stub.main.name.clone()
    };
    if destination_package_name == stub_target_package_name
        && !is_self_stub(&stub_wit, dest_wit_root)?
    {
        return Err(anyhow!(
            "Both the caller and the target components are using the same package name ({destination_package_name}), which is not supported."
        ));
    }

    let stub_package_name = parsed_stub.main.name.clone();

    let mut actions = Vec::new();

    // Checking if the destination package is the same as the stub's package - if yes, then this is the case of adding a self-dependency
    // (adding A-stub to A as a dependency)
    if is_package_same_or_stub(&parsed_dest.main, &parsed_stub.main) {
        let stub_root = &stub_wit_root
            .parent()
            .ok_or(anyhow!("Failed to get parent of stub wit root"))?;

        // We re-generate stub instead of copying it and inline types
        let stub_definition = StubDefinition::new(
            dest_wit_root,
            stub_root,
            &None,
            "0.0.1", // Version is unused when it comes to re-generating stub at this stage.
            &WasmRpcOverride::default(), // wasm-rpc path is unused when it comes to re-generating stub during dependency addition
            true,
        )?;

        // We filter the dependencies of stub that's already existing in dest_wit_root
        let filtered_source_deps = stub_deps
            .into_iter()
            .filter(|dep| find_if_same_package(dep, &parsed_dest.main).unwrap())
            .collect::<Vec<_>>();

        // New stub string
        let new_stub = get_stub_wit(&stub_definition, StubTypeGen::InlineRootTypes)
            .context("Failed to regenerate inlined stub")?;

        for source_dir in filtered_source_deps {
            actions.push(WitAction::CopyDepDir { source_dir })
        }

        actions.push(WitAction::WriteWit {
            source_wit: new_stub,
            dir_name: format!("{}_{}", stub_package_name.namespace, stub_package_name.name),
            file_name: "_stub.wit".to_string(),
        });
    } else {
        for source_dir in stub_deps {
            let parsed_dep = UnresolvedPackageGroup::parse_dir(&source_dir)?.main;

            if is_package_same_or_stub(&parsed_dest.main, &parsed_dep) {
                log_warn_action(
                    "Skipping",
                    format!("copying cyclic dependencies {}", parsed_dep.name),
                );
            } else {
                let entries = fs::read_dir(&source_dir)?;
                for entry in entries {
                    let dependency_path = entry?.path();
                    let updated_source =
                        remove_stub_import(&dependency_path, &destination_package_name)?;

                    let dependency_file_name = get_file_name(&dependency_path)?;
                    let dependency_directory_name = get_file_name(&source_dir)?;

                    actions.push(WitAction::WriteWit {
                        source_wit: updated_source,
                        dir_name: dependency_directory_name,
                        file_name: dependency_file_name,
                    });
                }
            }
        }

        actions.push(WitAction::CopyDepWit {
            source_wit: stub_wit,
            dir_name: format!("{}_{}", stub_package_name.namespace, stub_package_name.name),
        });
    }

    let mut proceed = true;
    for action in &actions {
        if !verify_action(action, dest_wit_root, overwrite)? {
            eprintln!("Cannot {action} because the destination already exists with a different content. Use --overwrite to force.");
            proceed = false;
        }
    }

    if proceed {
        for action in &actions {
            action.perform(dest_wit_root)?;
        }
    }

    if let Some(target_parent) = dest_wit_root.parent() {
        let target_cargo_toml = target_parent.join("Cargo.toml");
        if target_cargo_toml.exists() && target_cargo_toml.is_file() {
            if update_cargo_toml == UpdateCargoToml::NoUpdate {
                eprintln!("Warning: the newly copied dependencies have to be added to {}. Use the --update-cargo-toml flag to update it automatically.", target_cargo_toml.to_string_lossy());
            } else {
                cargo::is_cargo_component_toml(&target_cargo_toml).context(format!(
                    "The file {target_cargo_toml:?} is not a valid cargo-component project"
                ))?;
                let mut names = Vec::new();
                for action in actions {
                    names.push(action.get_dep_dir_name()?);
                }
                cargo::add_dependencies_to_cargo_toml(&target_cargo_toml, &names)?;
            }
        } else if update_cargo_toml == UpdateCargoToml::Update {
            return Err(anyhow!(
                "Cannot update {:?} file because it does not exist or is not a file",
                target_cargo_toml
            ));
        }
    } else if update_cargo_toml == UpdateCargoToml::Update {
        return Err(anyhow!("Cannot update the Cargo.toml file because parent directory of the destination WIT root does not exist."));
    }

    Ok(())
}

/// Checks whether `stub_wit` is a stub generated for `dest_wit_root`
fn is_self_stub(stub_wit: &Path, dest_wit_root: &Path) -> anyhow::Result<bool> {
    let temp_root = TempDir::new()?;
    let canonical_temp_root = temp_root.path().canonicalize()?;
    let dest_stub_def = StubDefinition::new(
        dest_wit_root,
        &canonical_temp_root,
        &None,
        "0.0.1",
        &WasmRpcOverride::default(),
        false,
    )?;
    let dest_stub_wit_imported = get_stub_wit(&dest_stub_def, StubTypeGen::ImportRootTypes)?;
    let dest_stub_wit_inlined = get_stub_wit(&dest_stub_def, StubTypeGen::InlineRootTypes)?;

    let stub_wit = std::fs::read_to_string(stub_wit)?;

    Ok(stub_wit == dest_stub_wit_imported || stub_wit == dest_stub_wit_inlined)
}

fn find_if_same_package(dep_dir: &Path, target_wit: &UnresolvedPackage) -> anyhow::Result<bool> {
    let dep_package_name = UnresolvedPackageGroup::parse_dir(dep_dir)?.main.name;
    let dest_package = target_wit.name.clone();

    if dep_package_name != dest_package {
        Ok(true)
    } else {
        log_warn_action(
            "Skipping",
            format!("copying cyclic dependencies {}", dep_package_name),
        );
        Ok(false)
    }
}

/// Checks whether `dependency`'s package name is either the same as `destination`'s, or
/// the same as the **stub package name** that would be generated for `destination`.
///
/// With this check it is possible to skip copying cyclic dependencies.
fn is_package_same_or_stub(
    destination: &UnresolvedPackage,
    dependency: &UnresolvedPackage,
) -> bool {
    let self_stub_name = format!("{}-stub", destination.name.name);

    dependency.name == destination.name
        || (dependency.name.namespace == destination.name.namespace
            && dependency.name.name == self_stub_name)
}

/// Removes all 'import pkg:name-stub/*;' statements from the given WIT file
///
/// This is used when there are circular references between the stubbed components.
/// When adding a stub dependency to a component A, these dependencies may have imports to A's stub.
/// We cannot keep these imports, because that would mean that A's own stub has to be also added to A
/// as a dependency. So we remove these imports from the worlds.
///
/// Note that this has no effect on the outcome because these import statements are in `world` sections
/// which are not used when these WITs are added as dependencies.
fn remove_stub_import(wit_file: &PathBuf, package_name: &PackageName) -> anyhow::Result<String> {
    let read_data = fs::read_to_string(wit_file)?;
    // TODO: naming
    let self_stub_package = format!("{}:{}-stub", package_name.namespace, package_name.name);

    let re = Regex::new(
        format!(
            r"import\s+{}(/[^;]*)?;",
            regex::escape(self_stub_package.as_str())
        )
        .as_str(),
    )?;

    Ok(re.replace_all(&read_data, "").to_string())
}

fn get_file_name(path: &Path) -> anyhow::Result<String> {
    let msg = format!(
        "Failed to get the file name of the dependency wit path {:?}",
        path
    );

    Ok(path
        .file_name()
        .ok_or(anyhow::Error::msg(msg.clone()))?
        .to_str()
        .ok_or(anyhow::Error::msg(msg))?
        .to_string())
}
