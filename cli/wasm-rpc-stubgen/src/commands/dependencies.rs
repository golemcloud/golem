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

use crate::commands::log::{log_action, log_action_plan, log_warn_action, LogIndent};
use crate::fs::{get_file_name, strip_path_prefix, OverwriteSafeAction, OverwriteSafeActions};
use crate::wit_encode::EncodedWitDir;
use crate::wit_generate::generate_stub_wit_from_wit_dir;
use crate::wit_resolve::ResolvedWitDir;
use crate::wit_transform::{add_import_to_all_world, remove_imports_from_package_all_worlds};
use crate::{cargo, naming};
use anyhow::{anyhow, bail, Context};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wit_parser::PackageName;

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
    log_action(
        "Adding",
        format!(
            "stub dependencies to {} from {}",
            dest_wit_root.display(),
            stub_wit_root.display()
        ),
    );

    let _indent = LogIndent::new();

    let stub_resolved_wit_root = ResolvedWitDir::new(stub_wit_root)?;
    let mut stub_encoded_wit_root = EncodedWitDir::new(&stub_resolved_wit_root.resolve)?;
    let stub_package = stub_resolved_wit_root.main_package()?;
    let stub_wit = stub_wit_root.join(naming::wit::STUB_WIT_FILE_NAME);

    let dest_deps_dir = dest_wit_root.join(naming::wit::DEPS_DIR);
    let dest_resolved_wit_root = ResolvedWitDir::new(dest_wit_root)?;
    let mut dest_encoded_wit_root = EncodedWitDir::new(&dest_resolved_wit_root.resolve)?;
    let dest_package = dest_resolved_wit_root.main_package()?;
    let dest_stub_package_name = naming::wit::stub_package_name(&dest_package.name);

    // TODO: have a better matcher which also considers / and @
    let dest_stub_package_import_prefix = dest_stub_package_name.to_string();
    let dest_package_import_prefix = dest_package.name.to_string();

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
    let mut package_names_to_package_path = BTreeMap::<PackageName, PathBuf>::new();

    for (package_name, package_id) in &stub_resolved_wit_root.resolve.package_names {
        let package_sources = stub_resolved_wit_root
            .package_sources
            .get(package_id)
            .ok_or_else(|| anyhow!("Failed to get package sources for {}", package_name))?;
        let package_path = naming::wit::package_wit_dep_dir_from_package_dir_name(&get_file_name(
            &package_sources.dir,
        )?);

        let is_stub_main_package = *package_id == stub_resolved_wit_root.package_id;
        let is_dest_package = *package_name == dest_package.name;
        let is_dest_stub_package = *package_name == dest_stub_package_name;

        // We skip self as a dependency
        if is_dest_package {
            log_warn_action(
                "Skipping",
                format!("cyclic self dependency for {}", package_name),
            );
        } else if is_dest_stub_package || is_stub_main_package {
            let package_dep_dir_name = naming::wit::package_dep_dir_name(package_name);
            let package_path = naming::wit::package_wit_dep_dir_from_package_name(package_name);

            package_names_to_package_path.insert(package_name.clone(), package_path);

            // Handle self stub packages: use regenerated stub with inlining, to break the recursive cycle
            if is_dest_stub_package {
                actions.add(OverwriteSafeAction::WriteFile {
                    content: generate_stub_wit_from_wit_dir(dest_wit_root, true)?,
                    target: dest_deps_dir
                        .join(&package_dep_dir_name)
                        .join(naming::wit::STUB_WIT_FILE_NAME),
                });
            // Non-self stub package has to be copied into target deps
            } else {
                for source in &package_sources.files {
                    actions.add(OverwriteSafeAction::CopyFile {
                        source: source.clone(),
                        target: dest_deps_dir
                            .join(&package_dep_dir_name)
                            .join(get_file_name(source)?),
                    });
                }
            }
        // Handle other package by copying while removing cyclic imports
        } else {
            package_names_to_package_path.insert(package_name.clone(), package_path);

            let package = stub_encoded_wit_root.package(*package_id)?;
            remove_imports_from_package_all_worlds(package, &dest_stub_package_import_prefix);
            remove_imports_from_package_all_worlds(package, &dest_package_import_prefix);
            let content = package.to_string();

            let first_source = package_sources.files.first().ok_or_else(|| {
                anyhow!(
                    "Expected at least one source for stub package: {}",
                    package_name
                )
            })?;
            let first_source_relative_path = strip_path_prefix(stub_wit_root, first_source)?;
            let target = {
                if package_sources.files.len() == 1 {
                    dest_wit_root.join(first_source_relative_path)
                } else {
                    dest_wit_root
                        .join(first_source_relative_path.parent().ok_or_else(|| {
                            anyhow!(
                                "Failed to get parent of stub source: {}",
                                first_source_relative_path.display()
                            )
                        })?)
                        .join(naming::wit::package_merged_wit_name(package_name))
                }
            };

            actions.add(OverwriteSafeAction::WriteFile { content, target });

            // TODO: still output old ones while experimenting
            for source in &package_sources.files {
                actions.add(OverwriteSafeAction::CopyFile {
                    source: source.clone(),
                    target: dest_wit_root.join(format!(
                        "{}.old",
                        strip_path_prefix(stub_wit_root, source)?.to_string_lossy()
                    )),
                });
            }
        }
    }

    // Import stub if overwrite enabled // TODO: use a different flag, or always import?
    if overwrite {
        let dest_main_package_id = dest_resolved_wit_root.package_id;

        let dest_main_package_sources = dest_resolved_wit_root
            .package_sources
            .get(&dest_main_package_id)
            .ok_or_else(|| anyhow!("Failed to get dest main package sources"))?;

        if dest_main_package_sources.files.len() != 1 {
            bail!(
                "Expected exactly one dest main package source, got sources: {:?}",
                dest_main_package_sources.files
            );
        }

        let package = dest_encoded_wit_root.package(dest_main_package_id)?;
        add_import_to_all_world(package, &naming::wit::stub_import_name(stub_package)?);
        let content = package.to_string();

        actions.add(OverwriteSafeAction::WriteFile {
            content,
            target: dest_main_package_sources.files[0].clone(),
        });
    }

    let forbidden_overwrites = actions.run(overwrite, log_action_plan)?;
    if !forbidden_overwrites.is_empty() {
        eprintln!("The following files would have been overwritten with new content:");
        for action in forbidden_overwrites {
            eprintln!("  {}", action.target().display());
        }
        eprintln!();
        eprintln!("Use --overwrite to force overwrite.");
    }

    if let Some(target_parent) = dest_wit_root.parent() {
        let target_cargo_toml = target_parent.join("Cargo.toml");
        if target_cargo_toml.exists() && target_cargo_toml.is_file() {
            if update_cargo_toml == UpdateCargoToml::NoUpdate {
                eprintln!("Warning: the newly copied dependencies have to be added to {}. Use the --update-cargo-toml flag to update it automatically.", target_cargo_toml.display());
            } else {
                cargo::is_cargo_component_toml(&target_cargo_toml).context(format!(
                    "The file {target_cargo_toml:?} is not a valid cargo-component project"
                ))?;
                cargo::add_dependencies_to_cargo_toml(
                    &target_cargo_toml,
                    package_names_to_package_path,
                )?;
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
