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

use crate::app::build::command::execute_build_command;
use crate::app::context::BuildContext;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};

pub async fn componentize(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    log_action("Building", "components");
    let _indent = LogIndent::new();

    for component_name in ctx.application_context().selected_component_names() {
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
            continue;
        }

        log_action(
            "Building",
            format!("{}", component_name.as_str().log_color_highlight()),
        );
        let _indent = LogIndent::new();

        for build_step in build_commands {
            execute_build_command(ctx, component_name, &build_step).await?;
        }
    }

    Ok(())
}
