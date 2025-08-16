use crate::app::build::delete_path_logged;
use crate::app::context::ApplicationContext;
use crate::fs::compile_and_collect_globs;
use crate::log::{log_action, LogColorize, LogIndent};
use crate::model::app::DependencyType;
use std::collections::BTreeSet;
use std::path::PathBuf;
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

// TODO: clean is not selected_component_names aware yet!
pub fn clean_app(ctx: &ApplicationContext) -> anyhow::Result<()> {
    {
        log_action("Cleaning", "components");
        let _indent = LogIndent::new();

        let all_profiles = ctx.application.all_option_build_profiles();
        let paths = {
            let mut paths = BTreeSet::<(&'static str, PathBuf)>::new();
            for component_name in ctx.application.component_names() {
                for profile in &all_profiles {
                    paths.insert((
                        "generated wit",
                        ctx.application
                            .component_generated_wit(component_name, profile.as_ref()),
                    ));
                    paths.insert((
                        "component wasm",
                        ctx.application
                            .component_wasm(component_name, profile.as_ref()),
                    ));
                    paths.insert((
                        "linked wasm",
                        ctx.application
                            .component_linked_wasm(component_name, profile.as_ref()),
                    ));

                    let properties = &ctx
                        .application
                        .component_properties(component_name, profile.as_ref());

                    for build_step in &properties.build {
                        let build_dir = build_step
                            .dir()
                            .map(|dir| {
                                ctx.application
                                    .component_source_dir(component_name)
                                    .join(dir)
                            })
                            .unwrap_or_else(|| {
                                ctx.application
                                    .component_source_dir(component_name)
                                    .to_path_buf()
                            });

                        paths.extend(
                            compile_and_collect_globs(&build_dir, &build_step.targets())?
                                .into_iter()
                                .map(|path| ("build output", path)),
                        );
                    }

                    paths.extend(properties.clean.iter().map(|path| {
                        (
                            "clean target",
                            ctx.application
                                .component_source_dir(component_name)
                                .join(path),
                        )
                    }));
                }
            }
            paths
        };

        for (context, path) in paths {
            delete_path_logged(context, &path)?;
        }
    }

    {
        log_action("Cleaning", "component clients");
        let _indent = LogIndent::new();

        for dep in ctx.application.all_dependencies() {
            if dep.dep_type.is_wasm_rpc() {
                if let Some(dep) = dep.as_dependent_app_component() {
                    log_action(
                        "Cleaning",
                        format!(
                            "component client {}",
                            dep.name.as_str().log_color_highlight()
                        ),
                    );
                    let _indent = LogIndent::new();

                    delete_path_logged("client wit", &ctx.application.client_wit(&dep.name))?;
                    if dep.dep_type == DependencyType::StaticWasmRpc {
                        delete_path_logged("client wasm", &ctx.application.client_wasm(&dep.name))?;
                    }
                }
            }
        }
    }

    {
        log_action("Cleaning", "common clean targets");
        let _indent = LogIndent::new();

        for clean in ctx.application.common_clean() {
            delete_path_logged("common clean target", &clean.source.join(&clean.value))?;
        }
    }

    {
        log_action("Cleaning", "application build dir");
        let _indent = LogIndent::new();

        delete_path_logged("temp dir", &ctx.application.temp_dir())?;
    }

    Ok(())
}
