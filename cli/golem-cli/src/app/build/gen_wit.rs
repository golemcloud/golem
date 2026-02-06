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

use crate::app::build::task_result_marker::{ComponentGeneratorMarkerHash, TaskResultMarker};
use crate::app::build::up_to_date_check::is_up_to_date;
use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, log_warn_action, LogColorize, LogIndent};
use crate::model::app::{BinaryComponentSource, DependencyType, DependentAppComponent};
use crate::wasm_rpc_stubgen::cargo::regenerate_cargo_package_component;
use crate::wasm_rpc_stubgen::wit_generate::{
    add_client_as_dependency_to_wit_dir, extract_exports_as_wit_dep,
    extract_wasm_interface_as_wit_dep, AddClientAsDepConfig, UpdateCargoToml,
};
use anyhow::{anyhow, Context, Error};
use golem_common::model::component::ComponentName;
use itertools::Itertools;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub async fn gen_wit(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    log_action("Generating", "WIT artifacts");
    let _indent = LogIndent::new();

    {
        for component_name in ctx.wit().await.component_order_cloned() {
            create_generated_base_wit(ctx, &component_name).await?;
        }

        let deps = ctx
            .application()
            .component_names()
            .flat_map(|component_name| {
                ctx.application()
                    .component_dependencies(component_name)
                    .iter()
                    .filter_map(|dep| {
                        dep.dep_type
                            .is_wasm_rpc()
                            .then(|| dep.as_dependent_app_component())
                            .flatten()
                    })
            })
            .collect::<Vec<_>>();

        for dep in &deps {
            if dep.dep_type.is_wasm_rpc() {
                build_client(ctx, dep).await?;
            }
        }
    }

    {
        let mut any_changed = false;
        let component_names = ctx
            .application()
            .component_names()
            .cloned()
            .collect::<Vec<_>>();
        for component_name in component_names {
            let changed = create_generated_wit(ctx, &component_name).await?;
            update_cargo_toml(ctx, changed, &component_name)?;
            any_changed |= changed;
        }
        if any_changed {
            ctx.application_context().update_wit_context().await?;
        }
    }

    Ok(())
}

async fn create_generated_base_wit(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component = ctx.application().component(component_name);
    let component_source_wit = component.source_wit();

    if component_source_wit.is_dir() {
        let inputs = {
            let mut inputs = ctx.application().wit_deps();
            inputs.push(component_source_wit.clone());
            inputs
        };
        let component_generated_base_wit = component.generated_base_wit();
        let task_result_marker = TaskResultMarker::new(
            &ctx.application().task_result_marker_dir(),
            ComponentGeneratorMarkerHash {
                component_name,
                generator_kind: "base_wit",
            },
        )?;

        if is_up_to_date(
            ctx.build_config().skip_up_to_date_checks
                || !task_result_marker.is_up_to_date()
                || !ctx.wit().await.is_dep_graph_up_to_date(component_name)?,
            || inputs,
            || [&component_generated_base_wit],
        ) {
            log_skipping_up_to_date(format!(
                "creating generated base wit directory for {}",
                component_name.as_str().log_color_highlight()
            ));
            Ok(false)
        } else {
            log_action(
                "Creating",
                format!(
                    "generated base wit directory for {}",
                    component_name.as_str().log_color_highlight(),
                ),
            );

            task_result_marker.result(
                (async {
                    let _indent = LogIndent::new();

                    fs::delete_path_logged(
                        "generated base wit directory",
                        &component_generated_base_wit,
                    )?;
                    copy_wit_sources(&component_source_wit, &component_generated_base_wit)?;

                    let mut missing_package_deps = ctx
                        .wit()
                        .await
                        .missing_generic_source_package_deps(component_name)?;
                    let mut packages_from_lib_deps = BTreeSet::new();

                    {
                        let library_dependencies = ctx
                            .application()
                            .component_dependencies(component_name)
                            .iter()
                            .filter(|dep| dep.dep_type == DependencyType::Wasm)
                            .collect::<BTreeSet<_>>();

                        if !library_dependencies.is_empty() {
                            log_action(
                                "Extracting",
                                format!(
                                    "WIT interface of library dependencies to {}",
                                    component_generated_base_wit.log_color_highlight()
                                ),
                            );
                            let _indent = LogIndent::new();
                            for library_dep in &library_dependencies {
                                // TODO: adding WIT packages from AppComponent wasm dependencies is not supported yet (we don't have a compiled WASM for them at this point)
                                if !matches!(
                                    library_dep.source,
                                    BinaryComponentSource::AppComponent { .. }
                                ) {
                                    let path = ctx.application_context().resolve_binary_component_source(library_dep).await?;
                                    let result = extract_wasm_interface_as_wit_dep(
                                        ctx.application_context().common_wit_deps()?,
                                        &library_dep.source.to_string(),
                                        &path,
                                        &component_generated_base_wit,
                                    )
                                        .with_context(|| {
                                            format!(
                                                "Failed to extract WIT interface of library dependency {}",
                                                library_dep.source.to_string().log_color_highlight()
                                            )
                                        })?;
                                    packages_from_lib_deps.extend(result.new_packages);
                                    missing_package_deps.extend(result.required_common_packages);
                                }
                            }
                        }
                    }

                    {
                        missing_package_deps.retain(|name| !packages_from_lib_deps.contains(name));

                        if !missing_package_deps.is_empty() {
                            log_action("Adding", "package deps");
                            let _indent = LogIndent::new();

                            ctx.application_context().common_wit_deps()
                                .with_context(|| {
                                    format!(
                                        "Failed to add package dependencies for {}, missing packages: {}",
                                        component_name.as_str().log_color_highlight(),
                                        missing_package_deps
                                            .iter()
                                            .map(|s| s.to_string().log_color_highlight())
                                            .join(", ")
                                    )
                                })?
                                .add_packages_with_transitive_deps_to_wit_dir(
                                    &missing_package_deps,
                                    &component_generated_base_wit,
                                )
                                .with_context(|| {
                                    format!(
                                        "Failed to add package dependencies for {} ({})",
                                        component_name.as_str().log_color_highlight(),
                                        component_source_wit.log_color_highlight()
                                    )
                                })?;
                        }
                    }

                    {
                        let component_exports_package_deps =
                            ctx.wit().await.component_exports_package_deps(component_name)?;
                        if !component_exports_package_deps.is_empty() {
                            log_action("Adding", "component exports package dependencies");
                            let _indent = LogIndent::new();

                            for (dep_exports_package_name, dep_component_name) in
                                &component_exports_package_deps
                            {
                                ctx.application_context().component_base_output_wit_deps(dep_component_name)?
                                    .add_packages_with_transitive_deps_to_wit_dir(
                                        std::slice::from_ref(dep_exports_package_name),
                                        &component_generated_base_wit,
                                    )?;
                            }
                        }
                    }

                    {
                        log_action(
                            "Extracting",
                            format!(
                                "exports package from {} to {}",
                                component_source_wit.log_color_highlight(),
                                component_generated_base_wit.log_color_highlight()
                            ),
                        );
                        let _indent = LogIndent::new();
                        extract_exports_as_wit_dep(&component_generated_base_wit)?
                    }

                    Ok(true)
                })
                    .await,
            )
        }
    } else {
        log_warn_action(
            "Skipping",
            format!(
                "creating generated base wit directory for {}, {}",
                component_name.as_str().log_color_highlight(),
                "source WIT points to a WASM component".log_color_ok_highlight()
            ),
        );
        Ok(false)
    }
}

async fn create_generated_wit(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> Result<bool, Error> {
    let component = ctx.application().component(component_name);
    let component_generated_base_wit = component.generated_base_wit();
    if component_generated_base_wit.exists() {
        let component_generated_wit = component.generated_wit();
        let task_result_marker = TaskResultMarker::new(
            &ctx.application().task_result_marker_dir(),
            ComponentGeneratorMarkerHash {
                component_name,
                generator_kind: "wit",
            },
        )?;

        if is_up_to_date(
            ctx.skip_up_to_date_checks()
                || !task_result_marker.is_up_to_date()
                || !ctx.wit().await.is_dep_graph_up_to_date(component_name)?,
            || [&component_generated_base_wit],
            || [&component_generated_wit],
        ) {
            log_skipping_up_to_date(format!(
                "creating generated wit directory for {}",
                component_name.as_str().log_color_highlight()
            ));
            Ok(false)
        } else {
            log_action(
                "Creating",
                format!(
                    "generated wit directory for {}",
                    component_name.as_str().log_color_highlight(),
                ),
            );

            task_result_marker.result((|| {
                let _indent = LogIndent::new();
                fs::delete_path_logged("generated wit directory", &component_generated_wit)?;
                copy_wit_sources(&component_generated_base_wit, &component_generated_wit)?;
                add_client_deps(ctx, component_name)?;
                Ok(true)
            })())
        }
    } else {
        log_warn_action(
            "Skipping",
            format!(
                "creating generated wit directory for {}, {}",
                component_name.as_str().log_color_highlight(),
                "no base WIT directory".log_color_ok_highlight()
            ),
        );
        Ok(false)
    }
}

fn update_cargo_toml(
    ctx: &BuildContext<'_>,
    mut skip_up_to_date_checks: bool,
    component_name: &ComponentName,
) -> anyhow::Result<()> {
    let component = ctx.application().component(component_name);
    let component_source_wit = component.source_wit();
    let component_source_wit_parent = fs::parent_or_err(&component_source_wit)?;
    let cargo_toml = component_source_wit_parent.join("Cargo.toml");

    if !cargo_toml.exists() {
        return Ok(());
    }

    let task_result_marker = TaskResultMarker::new(
        &ctx.application().task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name,
            generator_kind: "Cargo.toml",
        },
    )?;

    skip_up_to_date_checks |= skip_up_to_date_checks || ctx.skip_up_to_date_checks();
    if !skip_up_to_date_checks && task_result_marker.is_up_to_date() {
        log_skipping_up_to_date(format!(
            "updating Cargo.toml for {}",
            component_name.as_str().log_color_highlight()
        ));
        return Ok(());
    }

    task_result_marker.result(regenerate_cargo_package_component(
        &cargo_toml,
        &component.generated_wit(),
        None,
    ))
}

async fn build_client(
    _ctx: &BuildContext<'_>,
    _dependent_component: &DependentAppComponent,
) -> anyhow::Result<bool> {
    // TODO: WASM RPC cleanup
    todo!("WASM RPC client generation is deprecated")

    /*
    let stub_def = ctx
        .application_context()
        .component_stub_def(&dependent_component.name)?;
    let client_wit_root = stub_def.client_wit_root();

    let client_dep_package_ids = stub_def.stub_dep_package_ids();
    let client_sources: Vec<PathBuf> = stub_def
        .packages_with_wit_sources()
        .flat_map(|(package_id, _, sources)| {
            if client_dep_package_ids.contains(&package_id)
                || package_id == stub_def.source_package_id
            {
                sources.files.iter().cloned()
            } else {
                Default::default()
            }
        })
        .collect();

    let component = ctx.application().component(&dependent_component.name);
    let client_wasm = component.client_wasm();
    let client_wit = component.client_wit();

    let task_result_marker = TaskResultMarker::new(
        &ctx.application().task_result_marker_dir(),
        ComponentGeneratorMarkerHash {
            component_name: &dependent_component.name,
            generator_kind: "client",
        },
    )?;

    if is_up_to_date(
        ctx.skip_up_to_date_checks() || !task_result_marker.is_up_to_date(),
        || client_sources,
        || {
            if dependent_component.dep_type == DependencyType::StaticWasmRpc {
                vec![&client_wit, &client_wasm]
            } else {
                vec![&client_wit]
            }
        },
    ) {
        // TODO: message based on type
        log_skipping_up_to_date(format!(
            "generating WASM RPC client for {}",
            dependent_component.name.as_str().log_color_highlight()
        ));
        Ok(false)
    } else {
        task_result_marker.result(
            async {
                match dependent_component.dep_type {
                    DependencyType::StaticWasmRpc => {
                        log_action(
                            "Building",
                            format!(
                                "WASM RPC client for {}",
                                dependent_component.name.as_str().log_color_highlight()
                            ),
                        );

                        let _indent = LogIndent::new();

                        delete_path_logged("client temp build dir", &client_wit_root)?;
                        delete_path_logged("client wit", &client_wit)?;
                        delete_path_logged("client wasm", &client_wasm)?;

                        log_action(
                            "Creating",
                            format!(
                                "client temp build dir {}",
                                client_wit_root.log_color_highlight()
                            ),
                        );
                        fs::create_dir_all(&client_wit_root)?;

                        let offline = ctx.application_config().offline;
                        commands::generate::build(
                            ctx.application_context()
                                .component_stub_def(&dependent_component.name)?,
                            &client_wasm,
                            &client_wit,
                            offline,
                        )
                        .await?;

                        delete_path_logged("client temp build dir", &client_wit_root)?;

                        Ok(())
                    }
                    DependencyType::DynamicWasmRpc => {
                        log_action(
                            "Generating",
                            format!(
                                "WASM RPC client for {}",
                                dependent_component.name.as_str().log_color_highlight()
                            ),
                        );
                        let _indent = LogIndent::new();

                        delete_path_logged("client wit", &client_wit)?;

                        log_action(
                            "Creating",
                            format!(
                                "client temp build dir {}",
                                client_wit_root.log_color_highlight()
                            ),
                        );
                        fs::create_dir_all(&client_wit_root)?;

                        let stub_def = ctx
                            .application_context()
                            .component_stub_def(&dependent_component.name)?;
                        commands::generate::generate_and_copy_client_wit(stub_def, &client_wit)
                    }
                    DependencyType::Wasm => {
                        // No need to generate RPC clients for this dependency type
                        Ok(())
                    }
                }
            }
            .await,
        )?;

        Ok(true)
    }
    */
}

fn add_client_deps(ctx: &BuildContext<'_>, component_name: &ComponentName) -> Result<bool, Error> {
    let dependencies = ctx.application().component_dependencies(component_name);
    if dependencies.is_empty() {
        Ok(false)
    } else {
        log_action(
            "Adding",
            format!(
                "client wit dependencies to {}",
                component_name.as_str().log_color_highlight()
            ),
        );

        let _indent = LogIndent::new();

        let component = ctx.application().component(component_name);
        let component_generated_wit = component.generated_wit();

        for dep_component in dependencies {
            if dep_component.dep_type.is_wasm_rpc() {
                if let Some(dep_component) = dep_component.as_dependent_app_component() {
                    log_action(
                        "Adding",
                        format!(
                            "{} client wit dependency to {}",
                            dep_component.name.as_str().log_color_highlight(),
                            component_name.as_str().log_color_highlight()
                        ),
                    );
                    let _indent = LogIndent::new();

                    let dep_component = ctx.application().component(&dep_component.name);
                    add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
                        client_wit_root: dep_component.client_wit(),
                        dest_wit_root: component_generated_wit.clone(),
                        update_cargo_toml: UpdateCargoToml::NoUpdate,
                    })?
                }
            }
        }

        Ok(true)
    }
}

fn copy_wit_sources(source: &Path, target: &Path) -> anyhow::Result<()> {
    log_action(
        "Copying",
        format!(
            "wit sources from {} to {}",
            source.log_color_highlight(),
            target.log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();

    let dir_content = fs_extra::dir::get_dir_content(source).with_context(|| {
        anyhow!(
            "Failed to read component source wit directory entries for {}",
            source.log_color_highlight()
        )
    })?;

    for file in dir_content.files {
        let from = PathBuf::from(&file);
        let to = target.join(from.strip_prefix(source).with_context(|| {
            anyhow!(
                "Failed to strip prefix for source {}",
                &file.log_color_highlight()
            )
        })?);

        log_action(
            "Copying",
            format!(
                "wit source {} to {}",
                from.log_color_highlight(),
                to.log_color_highlight()
            ),
        );
        fs::copy(from, to)?;
    }

    Ok(())
}
