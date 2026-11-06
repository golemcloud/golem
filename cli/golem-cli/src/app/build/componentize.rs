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

use crate::app::build::command::execute_build_command;
use crate::app::build::task_result_marker::ComponentGeneratorMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::fs;
use crate::log::{LogColorize, LogIndent, log_action, log_skipping_up_to_date, log_warn_action};
use anyhow::Context;
use golem_common::model::component::ComponentName;

pub async fn build_components(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
) -> anyhow::Result<()> {
    let build_commands = ctx
        .application()
        .component(component_name)
        .build_commands()
        .clone();

    if build_commands.is_empty() {
        log_warn_action(
            "Skipping",
            format!(
                "building {}, no build steps",
                component_name.as_str().log_color_highlight(),
            ),
        );
    } else {
        log_action(
            "Building",
            format!("{}", component_name.as_str().log_color_highlight()),
        );
        let _indent = LogIndent::new();

        for build_step in build_commands {
            execute_build_command(ctx, component_name, &build_step).await?;
        }
    }

    let component = ctx.application().component(component_name);
    let source_wasm = component.wasm();
    let output_wasm = component.output_wasm();
    anyhow::ensure!(
        source_wasm.is_file(),
        "Component WASM is not a file: {}",
        source_wasm.display()
    );
    if output_wasm.exists()
        && same_file::is_same_file(&source_wasm, &output_wasm).with_context(|| {
            format!(
                "Failed to compare component WASM {} and output WASM {}",
                source_wasm.display(),
                output_wasm.display()
            )
        })?
    {
        return Ok(());
    }

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(ComponentGeneratorMarkerHash {
            component_name,
            generator_kind: "output-wasm",
        })?
        .with_sources(|| [&source_wasm])
        .with_targets(|| [&output_wasm])
        .run_or_skip(
            || {
                log_action(
                    "Copying",
                    format!("component WASM to {}", output_wasm.display()),
                );
                fs::copy(&source_wasm, &output_wasm)?;
                Ok(())
            },
            || log_skipping_up_to_date(format!("copying component WASM for {component_name}")),
        )
}
