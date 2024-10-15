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

use crate::commands::log::{log_action_plan, log_warn_action};
use crate::fs::{OverwriteSafeAction, OverwriteSafeActions};
use crate::wit::{generate_stub_wit_from_wit_dir, import_remover};
use crate::wit_resolve::ResolvedWitDir;
use crate::{cargo, naming};
use anyhow::{anyhow, Context};
use std::path::Path;

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
    let stub_resolved_wit_root = ResolvedWitDir::new(stub_wit_root)?;
    let stub_package = stub_resolved_wit_root.main_package()?;
    let stub_wit = stub_wit_root.join(naming::wit::STUB_WIT_FILE_NAME);

    let dest_deps_dir = dest_wit_root.join(naming::wit::DEPS_DIR);
    let dest_resolved_wit_root = ResolvedWitDir::new(dest_wit_root)?;
    let dest_package = dest_resolved_wit_root.main_package()?;
    let dest_stub_package_name = naming::wit::stub_package_name(&dest_package.name);
    let dest_stub_import_remover = import_remover(&dest_stub_package_name);

    {
        let is_self_stub_by_name =
            dest_package.name == naming::wit::stub_target_package_name(&stub_package.name);
        let is_self_stub_by_content = is_self_stub(&stub_wit, dest_wit_root);

        if is_self_stub_by_name && !is_self_stub_by_content? {
            return Err(anyhow!(
            "Both the caller and the target components are using the same package name ({}), which is not supported.",
            dest_package.name
        ));
        }
    }

    let mut actions = OverwriteSafeActions::new();
    for (package_name, package_id) in &stub_resolved_wit_root.resolve.package_names {
        let sources = stub_resolved_wit_root
            .sources
            .get(package_id)
            .ok_or_else(|| anyhow!("Failed to get package sources for {}", package_name))?
            .iter()
            .map(|source| source.to_path_buf())
            .collect::<Vec<_>>();

        let is_stub_main_package = *package_id == stub_resolved_wit_root.package_id;
        let is_dest_package = *package_name == dest_package.name;
        let is_dest_stub_package = *package_name == dest_stub_package_name;

        // We skip self as a dependency
        if is_dest_package {
            log_warn_action(
                "Skipping",
                format!("cyclic self dependency for {}", package_name),
            )
        // Handle self stub packages: use regenerated stub with inlining, to break the recursive cycle
        } else if is_dest_stub_package {
            let inlined_self_stub_wit = generate_stub_wit_from_wit_dir(dest_wit_root, true)?;

            let target_stub_wit = dest_deps_dir
                .join(naming::wit::package_dep_folder_name(package_name))
                .join(naming::wit::STUB_WIT_FILE_NAME);

            actions.add(OverwriteSafeAction::WriteFile {
                content: inlined_self_stub_wit,
                target: target_stub_wit,
            });
        // Non-self stub packages has to be copied into target deps
        } else if is_stub_main_package {
            let target_stub_dep_dir =
                dest_deps_dir.join(naming::wit::package_dep_folder_name(package_name));

            for source in sources {
                let file_name = source
                    .file_name()
                    .ok_or_else(|| {
                        anyhow!(
                            "Failed to get file name for package source: {}",
                            source.to_string_lossy(),
                        )
                    })?
                    .to_os_string();

                actions.add(OverwriteSafeAction::CopyFile {
                    source,
                    target: target_stub_dep_dir.join(file_name),
                });
            }
        // Handle other package by copying while removing imports
        } else {
            for source in sources {
                let relative_wit_path = source.strip_prefix(stub_wit_root).with_context(|| {
                    anyhow!(
                        "Failed to strip prefix for package source, stub wit root: {}, source: {}",
                        stub_wit_root.to_string_lossy(),
                        source.to_string_lossy()
                    )
                })?;
                let target = dest_wit_root.join(relative_wit_path);

                actions.add(OverwriteSafeAction::copy_file_transformed(
                    source.clone(),
                    target,
                    &dest_stub_import_remover,
                )?);
            }
        }
    }

    let targets = actions
        .targets()
        .iter()
        .map(|path| path.to_path_buf())
        .collect::<Vec<_>>();

    let forbidden_overwrites = actions.run(overwrite, log_action_plan)?;
    if !forbidden_overwrites.is_empty() {
        eprintln!("The following files would have been overwritten with new content:");
        for action in forbidden_overwrites {
            eprintln!("  {}", action.target().to_string_lossy());
        }
        eprintln!();
        eprintln!("Use --overwrite to force overwrite.");
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
                cargo::add_dependencies_to_cargo_toml(&target_cargo_toml, targets)?;
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
    // TODO: can we make it diff exports instead of generated content?
    let dest_stub_wit_imported = generate_stub_wit_from_wit_dir(dest_wit_root, false)?;
    let dest_stub_wit_inlined = generate_stub_wit_from_wit_dir(dest_wit_root, true)?;
    let stub_wit = std::fs::read_to_string(stub_wit)?;

    // TODO: this can also be false in case the stub is lagging
    Ok(stub_wit == dest_stub_wit_imported || stub_wit == dest_stub_wit_inlined)
}
