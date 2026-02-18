use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{log_action, LogIndent};
use crate::model::app::CleanMode;
use golem_common::model::component::ComponentName;
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

pub fn clean_app(ctx: &BuildContext<'_>, mode: CleanMode) -> anyhow::Result<()> {
    {
        log_action("Cleaning", "components");
        let _indent = LogIndent::new();

        let paths = {
            let mut paths = BTreeSet::<(&'static str, PathBuf)>::new();

            let component_names: Vec<ComponentName> = match mode {
                CleanMode::All => ctx.application().component_names().cloned().collect(),
                CleanMode::SelectedComponentsOnly => ctx
                    .application_context()
                    .selected_component_names()
                    .iter()
                    .cloned()
                    .collect(),
            };

            for component_name in &component_names {
                let component = ctx.application().component(component_name);
                let component_source_dir = component.source_dir();

                paths.insert(("generated wit", component.generated_wit()));
                paths.insert(("component wasm", component.wasm()));
                paths.insert(("output wasm", component.final_wasm()));

                for build_step in component.build_commands() {
                    let build_dir = build_step
                        .dir()
                        .map(|dir| component_source_dir.join(dir))
                        .unwrap_or_else(|| component_source_dir.to_path_buf());

                    paths.extend(
                        fs::compile_and_collect_globs(&build_dir, &build_step.targets())?
                            .into_iter()
                            .map(|path| ("build output", path)),
                    );
                }

                paths.extend(
                    component
                        .clean()
                        .iter()
                        .map(|path| ("clean target", component_source_dir.join(path))),
                );
            }
            paths
        };

        for (context, path) in paths {
            fs::delete_path_logged(context, &path)?;
        }
    }

    match mode {
        CleanMode::All => {
            log_action("Cleaning", "common clean targets");
            let _indent = LogIndent::new();

            for clean in ctx.application().common_clean() {
                fs::delete_path_logged("common clean target", &clean.source.join(&clean.value))?;
            }

            log_action("Cleaning", "application build dir");
            let _indent = LogIndent::new();

            fs::delete_path_logged("temp dir", ctx.application().temp_dir())?;
        }
        CleanMode::SelectedComponentsOnly => {
            // NOP
        }
    }

    Ok(())
}
