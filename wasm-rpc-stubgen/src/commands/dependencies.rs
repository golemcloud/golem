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

use crate::stub::StubDefinition;
use crate::wit::{get_stub_wit, verify_action, StubTypeGen, WitAction};
use crate::{cargo, wit, WasmRpcOverride};
use anyhow::{anyhow, Context};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use wit_parser::{PackageName, UnresolvedPackage};

pub fn add_stub_dependency(
    stub_wit_root: &Path,
    dest_wit_root: &Path,
    overwrite: bool,
    update_cargo_toml: bool,
) -> anyhow::Result<()> {
    // The destination's WIT's package details
    let destination_wit_root = UnresolvedPackage::parse_dir(dest_wit_root)?;

    // Dependencies of stub as directories
    let source_deps = wit::get_dep_dirs(stub_wit_root)?;

    let main_wit = stub_wit_root.join("_stub.wit");
    let parsed = UnresolvedPackage::parse_file(&main_wit)?;

    let destination_package_name = destination_wit_root.name.clone();
    let stub_target_package_name = PackageName {
        name: parsed
            .name
            .name
            .strip_suffix("-stub")
            .expect("Unexpected stub package name")
            .to_string(),
        ..parsed.name.clone()
    };
    if destination_package_name == stub_target_package_name {
        return Err(anyhow!(
            "Both the caller and the target components are using the same package name ({destination_package_name}), which is not supported."
        ));
    }

    let world_name = find_world_name(parsed)?;
    let mut actions = Vec::new();

    // If stub generated world points to the destination world (meaning the destination still owns the world for which the stub is generated),
    // we re-generation of stub with inlined types and copy the inlined stub to the destination
    if dest_owns_stub_world(&world_name, &destination_wit_root) {
        let stub_root = &stub_wit_root
            .parent()
            .ok_or(anyhow!("Failed to get parent of stub wit root"))?;

        // We re-generate stub instead of copying it and inline types
        let stub_definition = StubDefinition::new(
            dest_wit_root,
            stub_root,
            &Some(world_name),
            "0.0.1", // Version is unused when it comes to re-generating stub at this stage.
            &WasmRpcOverride::default(), // wasm-rpc path is unused when it comes to re-generating stub during dependency addition
            true,
        )?;

        // We filter the dependencies of stub that's already existing in dest_wit_root
        let filtered_source_deps = source_deps
            .into_iter()
            .filter(|dep| find_if_same_package(dep, &destination_wit_root).unwrap())
            .collect::<Vec<_>>();

        // New stub string
        let new_stub = get_stub_wit(&stub_definition, StubTypeGen::InlineRootTypes)
            .context("Failed to regenerate inlined stub")?;

        let main_wit_package_name = wit::get_package_name(&main_wit)?;

        for source_dir in filtered_source_deps {
            actions.push(WitAction::CopyDepDir { source_dir })
        }

        actions.push(WitAction::WriteWit {
            source_wit: new_stub,
            dir_name: format!(
                "{}_{}",
                main_wit_package_name.namespace, main_wit_package_name.name
            ),
            file_name: "_stub.wit".to_string(),
        });
    } else {
        let main_wit_package_name = wit::get_package_name(&main_wit)?;

        for source_dir in source_deps {
            let parsed = UnresolvedPackage::parse_dir(&source_dir)?;

            if is_invalid_dependency(&destination_wit_root, &parsed) {
                println!("Skipping the copy of cyclic dependencies {}", parsed.name);
            } else {
                let entries = fs::read_dir(&source_dir)?;
                for entry in entries {
                    let dependency_wit_path = entry?.path();
                    let source_wit = replace_self_imports_from_dependencies(
                        &dependency_wit_path,
                        &destination_wit_root,
                    )?;

                    let dependency_file_name = get_file_name(&dependency_wit_path)?;
                    let dependency_directory_name = get_file_name(&source_dir)?;

                    actions.push(WitAction::WriteWit {
                        source_wit,
                        dir_name: dependency_directory_name,
                        file_name: dependency_file_name,
                    });
                }
            }
        }

        actions.push(WitAction::CopyDepWit {
            source_wit: main_wit,
            dir_name: format!(
                "{}_{}",
                main_wit_package_name.namespace, main_wit_package_name.name
            ),
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
            if !update_cargo_toml {
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
        } else if update_cargo_toml {
            return Err(anyhow!(
                "Cannot update {:?} file because it does not exist or is not a file",
                target_cargo_toml
            ));
        }
    } else if update_cargo_toml {
        return Err(anyhow!("Cannot update the Cargo.toml file because parent directory of the destination WIT root does not exist."));
    }

    Ok(())
}

fn find_if_same_package(dep_dir: &Path, target_wit: &UnresolvedPackage) -> anyhow::Result<bool> {
    let dep_package_name = UnresolvedPackage::parse_dir(dep_dir)?.name;
    let dest_package = target_wit.name.clone();

    if dep_package_name != dest_package {
        Ok(true)
    } else {
        println!(
            "Skipping the copy of cyclic dependencies {}",
            dep_package_name
        );
        Ok(false)
    }
}

fn find_world_name(unresolved_package: UnresolvedPackage) -> anyhow::Result<String> {
    // In reality, there is only 1 interface in generated stub in 1 _stub.wit
    for (_, interface) in unresolved_package.interfaces {
        if let Some(name) = interface.name {
            if name.starts_with("stub-") {
                let world_name = name.replace("stub-", "");
                return Ok(world_name);
            }
        }
    }

    Err(anyhow!("Failed to find world name from the stub. The interface name in stub is expected to have the pattern stub-<world-name>"))
}

fn dest_owns_stub_world(stub_world_name: &str, destination_wit_root: &UnresolvedPackage) -> bool {
    destination_wit_root
        .worlds
        .iter()
        .map(|(_, world)| world.name.clone())
        .collect::<Vec<_>>()
        .contains(&stub_world_name.to_string())
}

// When copying the wit files of the target to the packages wit/deps in the source, we need to ensure
// these dependencies are not the source itself, or it's stub version
// For cases where adding the stub dependency to its own package is valid (i.e, in case of self-loop/ direct-cycle dependency)
// this function is/should-never-be called because, in this case destination owns the stub world (the stub to be copied) already
// and forms a different branch of logic.
fn is_invalid_dependency(
    destination_wit_root: &UnresolvedPackage,
    dependency_package: &UnresolvedPackage,
) -> bool {
    let self_stub_name = format!("{}-stub", destination_wit_root.name.name);

    dependency_package.name == destination_wit_root.name
        || (dependency_package.name.namespace == destination_wit_root.name.namespace
            && dependency_package.name.name == self_stub_name)
}

// For those dependencies we add to the source, if they are importing from the skipped/invalid dependencies
// we simply make sure to delete them
fn replace_self_imports_from_dependencies(
    dependency_wit_path: &PathBuf,
    destination_wit_root: &UnresolvedPackage,
) -> anyhow::Result<String> {
    let read_data = fs::read_to_string(dependency_wit_path)?;
    let self_stub_package = format!(
        "{}:{}-stub",
        destination_wit_root.name.namespace, destination_wit_root.name.name
    );

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
