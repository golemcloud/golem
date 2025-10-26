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
    AgentWrapperCommandMarkerHash, ComposeAgentWrapperCommandMarkerHash,
    GenerateQuickJSCrateCommandMarkerHash, GenerateQuickJSDTSCommandMarkerHash,
    InjectToPrebuiltQuickJsCommandMarkerHash, ResolvedExternalCommandMarkerHash, TaskResultMarker,
};
use crate::app::build::{delete_path_logged, is_up_to_date, new_task_up_to_date_check};
use crate::app::context::{ApplicationContext, ToolsWithEnsuredCommonDeps};
use crate::app::error::CustomCommandError;
use crate::fs::compile_and_collect_globs;
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, logln, LogColorize, LogIndent,
};
use crate::model::app::AppComponentName;
use crate::model::app_raw;
use crate::model::app_raw::{
    ComposeAgentWrapper, GenerateAgentWrapper, GenerateQuickJSCrate, GenerateQuickJSDTS,
    InjectToPrebuiltQuickJs,
};
use crate::wasm_rpc_stubgen::commands;
use crate::wasm_rpc_stubgen::commands::composition::Plug;
use anyhow::{anyhow, Context as AnyhowContext};
use camino::Utf8Path;
use colored::Colorize;
use gag::BufferRedirect;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, enabled, Level};
use wasm_rquickjs::{EmbeddingMode, JsModuleSpec};

pub async fn execute_build_command(
    ctx: &mut ApplicationContext,
    component_name: &AppComponentName,
    command: &app_raw::BuildCommand,
) -> anyhow::Result<()> {
    let base_build_dir = ctx
        .application
        .component_source_dir(component_name)
        .to_path_buf();
    match command {
        app_raw::BuildCommand::External(external_command) => {
            execute_external_command(ctx, &base_build_dir, external_command).await
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
            execute_compose_agent_wrapper(ctx, component_name, &base_build_dir, command).await
        }
        app_raw::BuildCommand::InjectToPrebuiltQuickJs(command) => {
            execute_inject_to_prebuilt_quick_js(ctx, &base_build_dir, command).await
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

    // NOTE: cannot use new_task_up_to_date_check yet, because of the mut app ctx
    let task_result_marker = TaskResultMarker::new(
        &ctx.application.task_result_marker_dir(),
        AgentWrapperCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        },
    )?;

    let skip_up_to_date_check =
        ctx.config.skip_up_to_date_checks || !task_result_marker.is_up_to_date();

    if is_up_to_date(
        skip_up_to_date_check,
        || [&compiled_wasm_path],
        || [&wrapper_wasm_path],
    ) {
        log_skipping_up_to_date(format!(
            "generating agent wrapper for {}",
            component_name.as_str().log_color_highlight()
        ));
        return Ok(());
    }

    log_action(
        "Generating",
        format!(
            "agent wrapper for {}",
            component_name.as_str().log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();

    let agent_types = ctx
        .wit
        .get_extracted_agent_types(component_name, compiled_wasm_path.as_std_path())
        .await;

    task_result_marker.result((|| {
        let agent_types = agent_types?;

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

        let redirect = (!enabled!(Level::WARN))
            .then(|| BufferRedirect::stderr().ok())
            .flatten();

        let result = crate::model::agent::moonbit::generate_moonbit_wrapper(
            wrapper_context,
            wrapper_wasm_path.as_std_path(),
        );

        if result.is_err() {
            if let Some(mut redirect) = redirect {
                let mut output = Vec::new();
                let read_result = redirect.read_to_end(&mut output);
                drop(redirect);
                read_result.expect("Failed to read stderr from moonbit redirect");
                std::io::stderr()
                    .write_all(output.as_slice())
                    .expect("Failed to write captured moonbit stderr");
            }
        }

        result
    })())
}

async fn execute_compose_agent_wrapper(
    ctx: &ApplicationContext,
    component_name: &AppComponentName,
    base_build_dir: &Path,
    command: &ComposeAgentWrapper,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wrapper_wasm_path = base_build_dir.join(&command.compose_agent_wrapper);
    let user_component = base_build_dir.join(&command.with_agent);
    let target_component = base_build_dir.join(&command.to);

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(ComposeAgentWrapperCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        })?
        .with_sources(|| [&wrapper_wasm_path, &user_component])
        .with_targets(|| [&target_component])
        .run_async_or_skip(
            || async {
                log_action(
                    "Composing",
                    format!(
                        "agent wrapper for {}",
                        component_name.to_string().log_color_highlight()
                    ),
                );
                let _indent = LogIndent::new();

                commands::composition::compose(
                    wrapper_wasm_path.as_std_path(),
                    vec![Plug {
                        name: user_component.to_string(),
                        wasm: user_component.as_std_path().to_path_buf(),
                    }],
                    target_component.as_std_path(),
                )
                .await
            },
            || {
                log_skipping_up_to_date(format!(
                    "composing agent wrapper for {}",
                    component_name.as_str().log_color_highlight()
                ));
            },
        )
        .await
}

async fn execute_inject_to_prebuilt_quick_js(
    ctx: &ApplicationContext,
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

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(InjectToPrebuiltQuickJsCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        })?
        .with_sources(|| [&base_wasm, &js_module, &js_module_wasm])
        .with_targets(|| [&target])
        .run_async_or_skip(
            || async {
                log_action(
                    "Injecting",
                    format!(
                        "JS module {} into QuickJS WASM {}",
                        js_module.log_color_highlight(),
                        base_wasm.log_color_highlight()
                    ),
                );
                let _indent = LogIndent::new();

                moonbit_component_generator::get_script::generate_get_script_component(
                    &js_module_contents,
                    &js_module_wasm,
                )?;

                commands::composition::compose(
                    base_wasm.as_std_path(),
                    vec![Plug {
                        name: "JS module".to_string(),
                        wasm: js_module_wasm.as_std_path().to_path_buf(),
                    }],
                    target.as_std_path(),
                )
                .await?;

                Ok(())
            },
            || {
                log_skipping_up_to_date(format!(
                    "injecting JS module {} into QuickJS WASM {}",
                    js_module.log_color_highlight(),
                    base_wasm.log_color_highlight()
                ));
            },
        )
        .await
}

pub async fn execute_custom_command(
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
            if let Err(error) = execute_external_command(ctx, &command.source, step).await {
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
                )
                .await
                {
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

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(GenerateQuickJSCrateCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        })?
        .with_sources(|| {
            std::iter::once(wit.as_std_path()).chain(js_paths.iter().map(|p| p.as_path()))
        })
        .with_targets(|| [&generate_quickjs_crate])
        .run_or_skip(
            || {
                log_action(
                    "Executing",
                    format!(
                        "WASM RQuickJS wrapper generator in directory {}",
                        base_build_dir.log_color_highlight()
                    ),
                );

                wasm_rquickjs::generate_wrapper_crate(
                    &wit,
                    &js_modules,
                    &generate_quickjs_crate,
                    command.world.as_deref(),
                )
            },
            || {
                log_skipping_up_to_date(format!(
                    "executing WASM RQuickJS wrapper generator in directory {}",
                    base_build_dir.log_color_highlight()
                ));
            },
        )
}

fn execute_quickjs_d_ts(
    ctx: &ApplicationContext,
    base_build_dir: &Path,
    command: &GenerateQuickJSDTS,
) -> anyhow::Result<()> {
    let base_build_dir = Utf8Path::from_path(base_build_dir).unwrap();
    let wit = &base_build_dir.join(&command.wit);
    let generate_quickjs_dts = &base_build_dir.join(&command.generate_quickjs_dts);

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(GenerateQuickJSDTSCommandMarkerHash {
            build_dir: base_build_dir.as_std_path(),
            command,
        })?
        .with_sources(|| [&wit])
        .with_targets(|| [&generate_quickjs_dts])
        .run_or_skip(
            || {
                log_action(
                    "Executing",
                    format!(
                        "WASM RQuickJS d.ts generator in directory {}",
                        base_build_dir.log_color_highlight()
                    ),
                );

                wasm_rquickjs::generate_dts(wit, generate_quickjs_dts, command.world.as_deref())
                    .context("Failed to generate QuickJS DTS")
                    .map(|_| ())
            },
            || {
                log_skipping_up_to_date(format!(
                    "executing WASM RQuickJS d.ts generator in directory {}",
                    base_build_dir.log_color_highlight()
                ));
            },
        )
}

pub async fn execute_external_command(
    ctx: &ApplicationContext,
    base_command_dir: &Path,
    command: &app_raw::ExternalCommand,
) -> anyhow::Result<()> {
    let build_dir = command
        .dir
        .as_ref()
        .map(|dir| base_command_dir.join(dir))
        .unwrap_or_else(|| base_command_dir.to_path_buf());
    if !std::fs::exists(&build_dir)? {
        log_action(
            "Creating",
            format!("directory {}", build_dir.log_color_highlight()),
        );
        std::fs::create_dir_all(&build_dir)?
    }

    // NOTE: cannot use new_task_up_to_date_check yet, because of the special source and target handling
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

    if !command.sources.is_empty() && !command.targets.is_empty() {
        let sources = compile_and_collect_globs(&build_dir, &command.sources)?;
        let targets = compile_and_collect_globs(&build_dir, &command.targets)?;

        if is_up_to_date(skip_up_to_date_checks, || sources, || targets) {
            log_skipping_up_to_date(format!(
                "executing external command '{}' in directory {}",
                command.command.log_color_highlight(),
                build_dir.log_color_highlight()
            ));
            return Ok(());
        }
    }

    log_action(
        "Executing",
        format!(
            "external command '{}' in directory {}",
            command.command.log_color_highlight(),
            build_dir.log_color_highlight()
        ),
    );

    task_result_marker.result(
        async {
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

            let command_tokens = shlex::split(&command.command).ok_or_else(|| {
                anyhow::anyhow!("Failed to parse external command: {}", command.command)
            })?;
            if command_tokens.is_empty() {
                return Err(anyhow!("Empty command!"));
            }

            ensure_common_deps_for_tool(
                &ctx.tools_with_ensured_common_deps,
                command_tokens[0].as_str(),
            )
            .await?;

            Command::new(command_tokens[0].clone())
                .args(command_tokens.iter().skip(1))
                .current_dir(build_dir)
                .stream_and_run(&command_tokens[0])
                .await
        }
        .await,
    )
}

pub async fn ensure_common_deps_for_tool(
    ctx: &ToolsWithEnsuredCommonDeps,
    tool: &str,
) -> anyhow::Result<()> {
    match tool {
        "node" | "npx" => {
            ctx.ensure_common_deps_for_tool_once("node", || async {
                if std::fs::exists("node_modules")? {
                    return Ok(());
                }

                log_warn_action(
                    "Detected",
                    format!(
                        "missing {}, executing {}",
                        "node_modules".log_color_highlight(),
                        "npm install".log_color_highlight()
                    ),
                );

                Command::new("npm")
                    .args(["install"])
                    .stream_and_run("npm")
                    .await
            })
            .await
        }
        "cargo" => {
            Command::new("cargo")
                .args(["component", "build"])
                .stream_and_run("cargo")
                .await
        }

        _ => Ok(())
    }
}

trait ExitStatusExt {
    fn check_exit_status(&self) -> anyhow::Result<()>;
}

impl ExitStatusExt for ExitStatus {
    fn check_exit_status(&self) -> anyhow::Result<()> {
        if self.success() {
            Ok(())
        } else {
            Err(anyhow!(format!(
                "Command failed with exit code: {}",
                self.code()
                    .map(|code| code.to_string().log_color_error_highlight().to_string())
                    .unwrap_or_else(|| "?".to_string())
            )))
        }
    }
}

trait CommandExt {
    async fn stream_and_wait_for_status(
        &mut self,
        command_name: &str,
    ) -> anyhow::Result<ExitStatus>;

    async fn stream_and_run(&mut self, command_name: &str) -> anyhow::Result<()> {
        self.stream_and_wait_for_status(command_name)
            .await?
            .check_exit_status()
    }

    fn stream_output(command_name: &str, child: &mut Child) -> anyhow::Result<()> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout for {command_name}"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stderr for {command_name}"))?;

        tokio::spawn({
            let prefix = format!("{} | ", command_name).green().bold();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logln(format!("{prefix} {line}"));
                }
            }
        });

        tokio::spawn({
            let prefix = format!("{} | ", command_name).red().bold();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logln(format!("{prefix} {line}"));
                }
            }
        });

        Ok(())
    }
}

impl CommandExt for Command {
    async fn stream_and_wait_for_status(
        &mut self,
        command_name: &str,
    ) -> anyhow::Result<ExitStatus> {
        let _indent = LogIndent::stash();

        let mut child = self
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn {command_name}"))?;

        Self::stream_output(command_name, &mut child)?;

        child
            .wait()
            .await
            .with_context(|| format!("Failed to execute {command_name}"))
    }
}
