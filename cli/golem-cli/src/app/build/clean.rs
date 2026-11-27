// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{LogIndent, log_action};
use crate::model::app::{Application, CleanMode};
use golem_common::model::component::ComponentName;
use std::collections::BTreeSet;
use std::path::PathBuf;

pub fn clean_app(ctx: &BuildContext<'_>, mode: CleanMode) -> anyhow::Result<()> {
    let operation_lock = Application::operation_lock_path(ctx.application().app_root_dir());
    let validate = |path: &std::path::Path| -> anyhow::Result<()> {
        let path = fs::absolute_lexical_path_from_base_dir(path, ctx.application().app_root_dir());
        anyhow::ensure!(
            !operation_lock.starts_with(&path),
            "Clean targets must not delete the application operation lock"
        );
        Ok(())
    };
    {
        log_action("Cleaning", "components");
        let _indent = LogIndent::new();

        let app_root_dir = ctx.application().app_root_dir();

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
                let component_source_dir = component.component_dir();

                paths.insert(("component wasm", component.wasm()));

                for build_step in component.build_commands() {
                    let build_dir = build_step
                        .dir()
                        .map(|dir| component_source_dir.join(dir))
                        .unwrap_or_else(|| component_source_dir.to_path_buf());

                    paths.extend(
                        fs::compile_and_collect_globs(
                            app_root_dir,
                            &build_dir,
                            &build_step.targets(),
                        )?
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

        for (_, path) in &paths {
            validate(path)?;
        }
        if matches!(mode, CleanMode::All) {
            for clean in ctx.application().common_clean() {
                validate(&clean.source.join(&clean.value))?;
            }
        }

        for (context, path) in paths {
            fs::delete_logged(context, &path)?;
        }
    }

    match mode {
        CleanMode::All => {
            log_action("Cleaning", "common clean targets");
            let _indent = LogIndent::new();

            for clean in ctx.application().common_clean() {
                fs::delete_logged("common clean target", &clean.source.join(&clean.value))?;
            }

            log_action("Cleaning", "application build dir");
            let _indent = LogIndent::new();

            let temp = ctx
                .application()
                .app_root_dir()
                .join(ctx.application().temp_dir());
            if temp.try_exists()? {
                // Waiters must keep using the same lock file across clean operations.
                for entry in std::fs::read_dir(&temp)? {
                    let entry = entry?;
                    if entry.path() != operation_lock {
                        fs::delete_logged("temp dir entry", &entry.path())?;
                    }
                }
            }
        }
        CleanMode::SelectedComponentsOnly => {
            // NOP
        }
    }

    Ok(())
}
