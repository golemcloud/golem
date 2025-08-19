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

use crate::app::build::task_result_marker::{
    GenerateQuickJSCrateCommandMarkerHash, GenerateQuickJSDTSCommandMarkerHash,
    ResolvedExternalCommandMarkerHash, TaskResultMarker,
};
use crate::app::build::{delete_path_logged, is_up_to_date, valid_env_vars};
use crate::app::context::ApplicationContext;
use crate::app::error::CustomCommandError;
use crate::fs::compile_and_collect_globs;
use crate::log::{log_action, log_skipping_up_to_date, LogColorize, LogIndent};
use crate::model::app::AppComponentName;
use crate::model::app_raw;
use crate::model::app_raw::{
    ComposeAgentWrapper, GenerateAgentWrapper, GenerateQuickJSCrate, GenerateQuickJSDTS,
    InjectToPrebuiltQuickJs,
};
use crate::wasm_rpc_stubgen::commands;
use anyhow::{anyhow, Context};
use camino::Utf8Path;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tracing::debug;
use wasm_rquickjs::{EmbeddingMode, JsModuleSpec};

pub async fn execute_build_command(
    ctx: &mut ApplicationContext,
    component_name: &AppComponentName,
    command: &app_raw::BuildCommand,
    additional_env_vars: HashMap<String, String>,
) -> anyhow::Result<()> {
    let base_build_dir = ctx
        .application
        .component_source_dir(component_name)
        .to_path_buf();
    match command {
        app_raw::BuildCommand::External(external_command) => {
            execute_external_command(ctx, &base_build_dir, external_command, additional_env_vars)
        }
        app_raw::BuildCommand::QuickJSCrate(command) => {
            execute_quickjs_create(ctx, &base_build_dir, command)
        }
        app_raw::BuildCommand::QuickJSDTS(command) => {
            execute_quickjs_d_ts(ctx, &base_build_dir, command)
        }
        app_raw::BuildCommand::AgentWrapper(command) => {
            execute_agent_wrapper(ctx, component_name, &base_build_dir, command).await
        }
        app_raw::BuildCommand::ComposeAgentWrapper(command) => {
            execute_compose_agent_wrapper(&base_build_dir, command).await
        }
        app_raw::BuildCommand::InjectToPrebuiltQuickJs(command) => {
            execute_inject_to_prebuilt_quick_js(&base_build_dir, command).await
        }
    }
}

async fn execute_agent_wrapper(
    ctx: &mut ApplicationContext,
    component_name: &AppComponentName,
    base_build_dir: &Path,
    command: &GenerateAgentWrapper,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wrapper_wasm_path = base_build_dir.join(&command.generate_agent_wrapper);
    let compiled_wasm_path = base_build_dir.join(&command.based_on_compiled_wasm);

    log_action(
        "Generating",
        format!(
            "agent wrapper for {}",
            component_name.to_string().log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();

    let agent_types = ctx
        .wit
        .get_extracted_agent_types(component_name, compiled_wasm_path.as_std_path())
        .await?;

    log_action(
        "Generating",
        format!(
            "agent WIT interface for {}",
            component_name.to_string().log_color_highlight()
        ),
    );

    let wrapper_context =
        crate::model::agent::wit::generate_agent_wrapper_wit(component_name, &agent_types)?;

    log_action(
        "Generating",
        format!(
            "agent WIT interface implementation to {}",
            wrapper_wasm_path.to_string().log_color_highlight()
        ),
    );

    crate::model::agent::moonbit::generate_moonbit_wrapper(
        wrapper_context,
        wrapper_wasm_path.as_std_path(),
    )?;

    Ok(())
}

async fn execute_compose_agent_wrapper(
    base_build_dir: &Path,
    command: &ComposeAgentWrapper,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wrapper_wasm_path = base_build_dir.join(&command.compose_agent_wrapper);
    let user_component = base_build_dir.join(&command.with_agent);
    let target_component = base_build_dir.join(&command.to);

    commands::composition::compose(
        wrapper_wasm_path.as_std_path(),
        &[user_component.as_std_path().to_path_buf()],
        target_component.as_std_path(),
    )
    .await?;

    Ok(())
}

async fn execute_inject_to_prebuilt_quick_js(
    base_build_dir: &Path,
    command: &InjectToPrebuiltQuickJs,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let base_wasm = base_build_dir.join(&command.inject_to_prebuilt_quickjs);
    let js_module = base_build_dir.join(&command.module);
    let js_module_contents = std::fs::read_to_string(&js_module)
        .with_context(|| format!("Failed to read JS module from {js_module}"))?;
    let js_module_wasm = base_build_dir.join(&command.module_wasm);
    let target = base_build_dir.join(&command.into);

    log_action(
        "Injecting",
        format!(
            "JS module {} into QuickJS WASM {}",
            js_module.log_color_highlight(),
            base_wasm.log_color_highlight()
        ),
    );

    moonbit_component_generator::get_script::generate_get_script_component(
        &js_module_contents,
        &js_module_wasm,
    )?;

    commands::composition::compose(
        base_wasm.as_std_path(),
        &[js_module_wasm.as_std_path().to_path_buf()],
        target.as_std_path(),
    )
    .await?;

    Ok(())
}

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

fn execute_quickjs_create(
    ctx: &ApplicationContext,
    base_build_dir: &Path,
    command: &GenerateQuickJSCrate,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wit = base_build_dir.join(&command.wit);
    let generate_quickjs_crate = base_build_dir.join(&command.generate_quickjs_crate);

    let mut js_modules = Vec::new();
    let mut js_paths = Vec::new();
    for (name, spec) in &command.js_modules {
        let mode = if spec == "@composition" {
            EmbeddingMode::Composition
        } else {
            let js = base_build_dir.join(spec);
            js_paths.push(js.clone().into_std_path_buf());
            EmbeddingMode::EmbedFile(js)
        };
        js_modules.push(JsModuleSpec {
            name: name.clone(),
            mode,
        });
    }

    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        GenerateQuickJSCrateCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        },
    )?;

    let skip_up_to_date_checks =
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date();

    if is_up_to_date(
        skip_up_to_date_checks,
        || [vec![wit.clone().into_std_path_buf()], js_paths].concat(),
        || vec![generate_quickjs_crate.clone().into_std_path_buf()],
    ) {
        log_skipping_up_to_date(format!(
            "executing WASM RQuickJS wrapper generator in directory {}",
            base_build_dir.log_color_highlight()
        ));
        return Ok(());
    }

    log_action(
        "Executing",
        format!(
            "WASM RQuickJS wrapper generator in directory {}",
            base_build_dir.log_color_highlight()
        ),
    );

    task_result_marker.result({
        wasm_rquickjs::generate_wrapper_crate(
            &wit,
            &js_modules,
            &generate_quickjs_crate,
            command.world.as_deref(),
        )
    })
}

fn execute_quickjs_d_ts(
    ctx: &ApplicationContext,
    base_build_dir: &Path,
    command: &GenerateQuickJSDTS,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wit = &base_build_dir.join(&command.wit);
    let generate_quickjs_dts = &base_build_dir.join(&command.generate_quickjs_dts);

    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        GenerateQuickJSDTSCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        },
    )?;

    let skip_up_to_date_checks =
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date();

    if is_up_to_date(
        skip_up_to_date_checks,
        || vec![wit.clone().into_std_path_buf()],
        || vec![generate_quickjs_dts.clone().into_std_path_buf()],
    ) {
        log_skipping_up_to_date(format!(
            "executing WASM RQuickJS d.ts generator in directory {}",
            base_build_dir.log_color_highlight()
        ));
        return Ok(());
    }

    log_action(
        "Executing",
        format!(
            "WASM RQuickJS d.ts generator in directory {}",
            base_build_dir.log_color_highlight()
        ),
    );

    task_result_marker.result({
        wasm_rquickjs::generate_dts(wit, generate_quickjs_dts, command.world.as_deref())
            .context("Failed to generate QuickJS DTS")
    })
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
