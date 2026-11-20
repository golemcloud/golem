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
use crate::app::build::plan::{BuildOperation, BuildProgress, BuildStage};
use crate::app::context::BuildContext;
use crate::log::{LogColorize, LogIndent, log_action, log_warn_action};

pub async fn build_components(
    ctx: &BuildContext<'_>,
    stage: &BuildStage,
    progress: &mut BuildProgress,
) -> anyhow::Result<()> {
    let component_name = &stage.component;
    let build_commands = stage
        .actions
        .iter()
        .filter_map(|action| match &action.operation {
            BuildOperation::Command(command) => Some(command),
            _ => None,
        })
        .collect::<Vec<_>>();

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

        let mut preparations = Vec::new();
        let mut command_index = 0;
        for action in &stage.actions {
            match &action.operation {
                BuildOperation::Dependencies(preparation) => preparations.push(preparation.clone()),
                BuildOperation::Command(command) => {
                    if command_index
                        >= progress
                            .executed_commands
                            .get(component_name)
                            .copied()
                            .unwrap_or_default()
                    {
                        execute_build_command(ctx, component_name, command, &preparations).await?;
                        progress
                            .executed_commands
                            .insert(component_name.clone(), command_index + 1);
                    }
                    command_index += 1;
                    preparations.clear();
                }
                _ => {}
            }
        }
    }

    let component = ctx.application().component(component_name);
    let wasm = component.wasm();
    anyhow::ensure!(
        wasm.is_file(),
        "Component WASM is not a file: {}",
        wasm.display()
    );

    Ok(())
}
