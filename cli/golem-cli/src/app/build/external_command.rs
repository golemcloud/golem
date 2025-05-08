// Copyright 2024-2025 Golem Cloud
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

use crate::app::build::task_result_marker::{ResolvedExternalCommandMarkerHash, TaskResultMarker};
use crate::app::build::{delete_path_logged, is_up_to_date, valid_env_vars};
use crate::app::context::ApplicationContext;
use crate::app::error::CustomCommandError;
use crate::fs::compile_and_collect_globs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::model::app_raw;
use anyhow::{anyhow, Context};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tracing::debug;

pub fn execute_custom_command(
    ctx: &ApplicationContext,
    command_name: &str,
) -> Result<(), CustomCommandError> {
    let all_custom_commands = ctx.application.all_custom_commands(ctx.build_profile());
    if !all_custom_commands.contains(command_name) {
        return Err(CustomCommandError::CommandNotFound);
    }

    log_action(
        "Executing",
        format!("custom command {}", command_name.log_color_highlight()),
    );
    let _indent = LogIndent::new();

    let common_custom_commands = ctx.application.common_custom_commands();
    if let Some(command) = common_custom_commands.get(command_name) {
        log_action(
            "Executing",
            format!(
                "common custom command {}",
                command_name.log_color_highlight(),
            ),
        );
        let _indent = LogIndent::new();

        for step in &command.value {
            if let Err(error) = execute_external_command(ctx, &command.source, step, HashMap::new())
            {
                return Err(CustomCommandError::CommandError { error });
            }
        }
    }

    for component_name in ctx.application.component_names() {
        let properties = &ctx
            .application
            .component_properties(component_name, ctx.build_profile());
        if let Some(custom_command) = properties.custom_commands.get(command_name) {
            log_action(
                "Executing",
                format!(
                    "custom command {} for component {}",
                    command_name.log_color_highlight(),
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = LogIndent::new();

            for step in custom_command {
                if let Err(error) = execute_external_command(
                    ctx,
                    ctx.application.component_source_dir(component_name),
                    step,
                    HashMap::new(),
                ) {
                    return Err(CustomCommandError::CommandError { error });
                }
            }
        }
    }

    Ok(())
}

pub fn execute_external_command(
    ctx: &ApplicationContext,
    base_build_dir: &Path,
    command: &app_raw::ExternalCommand,
    additional_env_vars: HashMap<String, String>,
) -> anyhow::Result<()> {
    let build_dir = command
        .dir
        .as_ref()
        .map(|dir| base_build_dir.join(dir))
        .unwrap_or_else(|| base_build_dir.to_path_buf());

    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        ResolvedExternalCommandMarkerHash {
            build_dir: &build_dir,
            command,
        },
    )?;

    let skip_up_to_date_checks =
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date();

    debug!(
        command = ?command,
        "execute external command"
    );

    let env_vars = {
        let mut map = HashMap::new();
        map.extend(valid_env_vars());
        map.extend(additional_env_vars);
        map
    };

    let command_string = envsubst::substitute(&command.command, &env_vars)
        .context("Failed to substitute env vars in command")?;

    if !command.sources.is_empty() && !command.targets.is_empty() {
        let sources = compile_and_collect_globs(&build_dir, &command.sources)?;
        let targets = compile_and_collect_globs(&build_dir, &command.targets)?;

        if is_up_to_date(skip_up_to_date_checks, || sources, || targets) {
            log_skipping_up_to_date(format!(
                "executing external command '{}' in directory {}",
                command_string.log_color_highlight(),
                build_dir.log_color_highlight()
            ));
            return Ok(());
        }
    }

    log_action(
        "Executing",
        format!(
            "external command '{}' in directory {}",
            command_string.log_color_highlight(),
            build_dir.log_color_highlight()
        ),
    );

    task_result_marker.result((|| {
        if !command.rmdirs.is_empty() {
            let _ident = LogIndent::new();
            for dir in &command.rmdirs {
                let dir = build_dir.join(dir);
                delete_path_logged("directory", &dir)?;
            }
        }

        if !command.mkdirs.is_empty() {
            let _ident = LogIndent::new();
            for dir in &command.mkdirs {
                let dir = build_dir.join(dir);
                if !std::fs::exists(&dir)? {
                    log_action(
                        "Creating",
                        format!("directory {}", dir.log_color_highlight()),
                    );
                    std::fs::create_dir_all(dir)?
                }
            }
        }

        let command_tokens = shlex::split(&command_string).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse external command: {}", command_string)
        })?;
        if command_tokens.is_empty() {
            return Err(anyhow!("Empty command!"));
        }

        let result = Command::new(command_tokens[0].clone())
            .args(command_tokens.iter().skip(1))
            .current_dir(build_dir)
            .status()
            .with_context(|| "Failed to execute command".to_string())?;

        if result.success() {
            Ok(())
        } else {
            Err(anyhow!(format!(
                "Command failed with exit code: {}",
                result
                    .code()
                    .map(|code| code.to_string().log_color_error_highlight().to_string())
                    .unwrap_or_else(|| "?".to_string())
            )))
        }
    })())
}
