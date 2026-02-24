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
    InjectToPrebuiltQuickJsCommandMarkerHash, ResolvedExternalCommandMarkerHash,
};
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::{BuildContext, ToolsWithEnsuredCommonDeps};
use crate::app::error::CustomCommandError;
use crate::composition::{compose, Plug};
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, log_warn_action, LogColorize, LogIndent};
use crate::model::app_raw;
use crate::model::app_raw::{GenerateQuickJSCrate, GenerateQuickJSDTS, InjectToPrebuiltQuickJs};
use crate::process::{with_hidden_output_unless_error, CommandExt, HiddenOutput};
use anyhow::{anyhow, Context as AnyhowContext};
use camino::Utf8Path;
use golem_common::model::component::ComponentName;
use std::path::Path;
use tokio::process::Command;
use tracing::{enabled, Level};
use wasm_rquickjs::{EmbeddingMode, JsModuleSpec};

pub async fn execute_build_command(
    ctx: &BuildContext<'_>,
    component_name: &ComponentName,
    command: &app_raw::BuildCommand,
) -> anyhow::Result<()> {
    let base_build_dir = ctx
        .application()
        .component(component_name)
        .source_dir()
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
        app_raw::BuildCommand::InjectToPrebuiltQuickJs(command) => {
            execute_inject_to_prebuilt_quick_js(ctx, &base_build_dir, command).await
        }
    }
}

async fn execute_inject_to_prebuilt_quick_js(
    ctx: &BuildContext<'_>,
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

                with_hidden_output_unless_error(
                    HiddenOutput::hide_stderr_if(!enabled!(Level::WARN)),
                    || {
                        moonbit_component_generator::get_script::generate_get_script_component(
                            &js_module_contents,
                            &js_module_wasm,
                        )
                    },
                )?;

                compose(
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
    ctx: &BuildContext<'_>,
    command_name: &str,
) -> Result<(), CustomCommandError> {
    let all_custom_commands = ctx.application().all_custom_commands();
    if !all_custom_commands.contains(command_name) {
        return Err(CustomCommandError::CommandNotFound);
    }

    log_action(
        "Executing",
        format!("custom command {}", command_name.log_color_highlight()),
    );
    let _indent = LogIndent::new();

    let common_custom_commands = ctx.application().common_custom_commands();
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

    for component_name in ctx.application().component_names() {
        let component = &ctx.application().component(component_name);
        if let Some(custom_command) = component.custom_commands().get(command_name) {
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
                if let Err(error) =
                    execute_external_command(ctx, component.source_dir(), step).await
                {
                    return Err(CustomCommandError::CommandError { error });
                }
            }
        }
    }

    Ok(())
}

fn execute_quickjs_create(
    ctx: &BuildContext<'_>,
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
    ctx: &BuildContext<'_>,
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
    ctx: &BuildContext<'_>,
    base_command_dir: &Path,
    command: &app_raw::ExternalCommand,
) -> anyhow::Result<()> {
    let build_dir = command
        .dir
        .as_ref()
        .map(|dir| base_command_dir.join(dir))
        .unwrap_or_else(|| base_command_dir.to_path_buf());

    let (sources, targets) = {
        if !command.sources.is_empty() && !command.targets.is_empty() {
            (
                fs::compile_and_collect_globs(&build_dir, &command.sources)?,
                fs::compile_and_collect_globs(&build_dir, &command.targets)?,
            )
        } else {
            (vec![], vec![])
        }
    };

    new_task_up_to_date_check(ctx)
        .with_task_result_marker(ResolvedExternalCommandMarkerHash {
            build_dir: &build_dir,
            command,
        })?
        .with_sources(|| sources)
        .with_targets(|| targets)
        .run_async_or_skip(
            || async {
                log_action(
                    "Executing",
                    format!(
                        "external command '{}' in directory {}",
                        command.command.log_color_highlight(),
                        build_dir.log_color_highlight()
                    ),
                );

                if !std::fs::exists(&build_dir)? {
                    log_action(
                        "Creating",
                        format!("directory {}", build_dir.log_color_highlight()),
                    );
                    std::fs::create_dir_all(&build_dir)?
                }

                if !command.rmdirs.is_empty() {
                    let _ident = LogIndent::new();
                    for dir in &command.rmdirs {
                        let dir = build_dir.join(dir);
                        fs::delete_path_logged("directory", &dir)?;
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
                    ctx.tools_with_ensured_common_deps(),
                    command_tokens[0].as_str(),
                )
                .await?;

                Command::new(command_tokens[0].clone())
                    .args(command_tokens.iter().skip(1))
                    .current_dir(&build_dir)
                    .stream_and_run(&command_tokens[0])
                    .await
            },
            || {
                log_skipping_up_to_date(format!(
                    "executing external command '{}' in directory {}",
                    command.command.log_color_highlight(),
                    build_dir.log_color_highlight()
                ));
            },
        )
        .await
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
        _ => Ok(()),
    }
}
