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

use crate::app::build::task_result_marker::GenerateBridgeReplMarkerHash;
use crate::app::build::up_to_date_check::new_task_up_to_date_check;
use crate::app::context::BuildContext;
use crate::bridge_gen::bridge_client_directory_name;
use crate::command_handler::repl::load_repl_metadata;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::log::{log_action, log_skipping_up_to_date, logln, LogIndent};
use crate::model::app::BuildConfig;
use crate::model::repl::{BridgeReplArgs, ReplMetadata, ReplScriptSource};
use crate::process::{CommandExt, ExitStatusExt};
use crate::{command_name, fs};
use golem_templates::model::GuestLanguage;
use heck::ToLowerCamelCase;
use indoc::formatdoc;
use itertools::Itertools;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::process::Command;

pub struct TypeScriptRepl {
    ctx: Arc<Context>,
}

impl TypeScriptRepl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn run(&self, args: BridgeReplArgs) -> anyhow::Result<()> {
        {
            log_action("Preparing", "TypeScript REPL");
            let _indent = LogIndent::new();

            self.generate_repl_package(&args).await?;
        }

        let mut npx_args = vec!["tsx".to_string(), "repl.ts".to_string()];

        if args.disable_auto_imports {
            npx_args.push("--disable-auto-imports".to_string());
        }

        if let Some(script) = &args.script {
            match script {
                ReplScriptSource::Inline(script) => {
                    npx_args.push("--script".to_string());
                    npx_args.push(script.clone());
                }
                ReplScriptSource::FromFile(path) => {
                    npx_args.push("--script-file".to_string());
                    npx_args.push(fs::path_to_str(path)?.to_string());
                }
            }
        }

        loop {
            let result = Command::new("npx")
                .args(&npx_args)
                .current_dir(&args.repl_root_dir)
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .stdin(std::process::Stdio::inherit())
                .envs(self.ctx.repl_handler().repl_server_env_vars().await?)
                .spawn()?
                .wait()
                .await?;

            if args.script.is_some() {
                return result.check_exit_status();
            }

            if result.code() != Some(75) {
                return result.check_exit_status();
            }

            {
                logln("");
                log_action("Reloading", "TypeScript REPL");
                let _indent = LogIndent::new();
                self.generate_repl_package(&args).await?;
                logln("");
            }
        }
    }

    async fn generate_repl_package(&self, args: &BridgeReplArgs) -> anyhow::Result<()> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let repl_metadata = load_repl_metadata(app_ctx, GuestLanguage::TypeScript).await?;
        let package_json_path = args.repl_root_dir.join("package.json");
        let tsconfig_json_path = args.repl_root_dir.join("tsconfig.json");
        let repl_ts_path = args.repl_root_dir.join("repl.ts");
        let relative_bridge_sdk_unix_path = fs::path_to_str(fs::strip_prefix_or_err(
            &args.repl_root_bridge_sdk_dir,
            &args.repl_root_dir,
        )?)?
        .replace("\\", "/");

        new_task_up_to_date_check(&BuildContext::new(app_ctx, &BuildConfig::new()))
            .with_task_result_marker(GenerateBridgeReplMarkerHash {
                language: GuestLanguage::TypeScript,
                agent_type_names: repl_metadata.agents.keys().sorted().as_slice(),
            })?
            .with_sources(|| {
                repl_metadata
                    .agents
                    .values()
                    .map(|agent| agent.client_dir.as_path())
            })
            .with_targets(|| vec![&package_json_path, &tsconfig_json_path, &repl_ts_path])
            .run_async_or_skip(
                async || {
                    log_action("Generating", "TypeScript REPL package");
                    let _indent = LogIndent::new();

                    self.generate_package_json(
                        &repl_metadata,
                        &relative_bridge_sdk_unix_path,
                        &package_json_path,
                    )?;
                    self.generate_tsconfig_json(
                        &relative_bridge_sdk_unix_path,
                        &tsconfig_json_path,
                    )?;
                    self.generate_repl_ts(&args, &repl_metadata, &repl_ts_path)?;

                    Command::new("npm")
                        .arg("install")
                        .current_dir(&args.repl_root_dir)
                        .stream_and_wait_for_status("npm")
                        .await?
                        .check_exit_status()?;

                    Ok(())
                },
                || {
                    log_skipping_up_to_date("generating TypeScript REPL package");
                },
            )
            .await
    }

    fn generate_package_json(
        &self,
        repl_metadata: &ReplMetadata,
        relative_bridge_sdk_unix_path: &str,
        package_json_path: &Path,
    ) -> anyhow::Result<()> {
        let workspaces = repl_metadata
            .agents
            .keys()
            .map(|agent_type_name| {
                format!(
                    "{}/{}",
                    relative_bridge_sdk_unix_path,
                    bridge_client_directory_name(agent_type_name)
                )
            })
            .collect::<Vec<_>>();

        let dependencies = repl_metadata
            .agents
            .keys()
            .map(|agent_type_name| (bridge_client_directory_name(agent_type_name), "*"))
            .collect::<BTreeMap<_, _>>();

        let package_json = json!({
          "name": "repl",
          "type": "module",
          "private": true,
          "workspaces": workspaces,
          "dependencies": dependencies,
          "devDependencies": {
            "@golem/golem-ts-repl": self.ctx.template_sdk_overrides().ts_package_version_or_path("golem-ts-repl"),
            "tsx": "^4.7",
            "typescript": "^5.9"
          }
        });

        fs::write_str(
            package_json_path,
            serde_json::to_string_pretty(&package_json)?,
        )
    }

    fn generate_tsconfig_json(
        &self,
        relative_bridge_sdk_unix_path: &str,
        tsconfig_json_path: &Path,
    ) -> anyhow::Result<()> {
        let tsconfig_json = json!({
          "compilerOptions": {
            "composite": true,
            "declaration": true,
            "esModuleInterop": true,
            "forceConsistentCasingInFileNames": true,
            "module": "NodeNext",
            "moduleResolution": "nodenext",
            "skipLibCheck": true,
            "sourceMap": true,
            "strict": true,
            "target": "ES2022"
          },
          "include": [
            "repl.ts",
            format!("{}/**/*.ts", relative_bridge_sdk_unix_path)
          ]
        });

        fs::write_str(
            tsconfig_json_path,
            serde_json::to_string_pretty(&tsconfig_json)?,
        )
    }

    fn generate_repl_ts(
        &self,
        args: &BridgeReplArgs,
        repl_metadata: &ReplMetadata,
        repl_ts_path: &Path,
    ) -> anyhow::Result<()> {
        let agents_config = repl_metadata
            .agents
            .keys()
            .map(|agent_type_name| {
                let client_package_name = bridge_client_directory_name(agent_type_name);
                let client_package_imported_name = client_package_name.to_lower_camel_case();
                Ok::<String, anyhow::Error>(formatdoc! {"
                    '{agent_type_name}': {{
                      clientPackageName: {client_package_name},
                      clientPackageImportedName: {client_package_imported_name},
                      package: await import({client_package_name}),
                    }}",
                    client_package_name = js_string_literal(client_package_name)?,
                    client_package_imported_name = js_string_literal(client_package_imported_name)?
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(",\n")
            .lines()
            .enumerate()
            .map(|(idx, l)| {
                if idx == 0 {
                    l.to_string()
                } else {
                    format!("    {}", l)
                }
            })
            .join("\n");

        let repl_ts = formatdoc! {"
                import 'tsx/patch-repl';
                const {{ Repl }} = await import('@golem/golem-ts-repl');

                const repl = new Repl({{
                  binary: {binary},
                  appMainDir: {app_main_dir},
                  agents: {{
                  {agents_config}
                  }},
                  historyFile: {repl_history_file_path},
                  cliCommandsMetadataJsonPath: {repl_cli_commands_metadata_json_path},
                }});

                void repl.run();
            ",
            binary = js_string_literal(command_name())?,
            app_main_dir = js_string_literal(args.app_main_dir.display().to_string())?,
            repl_history_file_path = js_string_literal(args.repl_history_file_path.display().to_string())?,
            repl_cli_commands_metadata_json_path =
                js_string_literal(args.repl_cli_commands_metadata_json_path.display().to_string())?,
        };

        fs::write_str(repl_ts_path, repl_ts)
    }
}

fn js_string_literal(s: impl AsRef<str>) -> anyhow::Result<String> {
    Ok(serde_json::to_string(s.as_ref())?)
}
