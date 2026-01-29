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
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date, logln, LogIndent};
use crate::model::app::BuildConfig;
use crate::model::repl::BridgeReplArgs;
use crate::process::{CommandExt, ExitStatusExt};
use golem_templates::model::GuestLanguage;
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

            if !args.repl_root_dir.join("node_modules").exists() {
                Command::new("npm")
                    .arg("install")
                    .current_dir(&args.repl_root_dir)
                    .stream_and_wait_for_status("TS REPL - npm install")
                    .await?
                    .check_exit_status()?;
            }
        }

        loop {
            let result = Command::new("npx")
                .args(&["tsx", "repl.ts"])
                .current_dir(&args.repl_root_dir)
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .stdin(std::process::Stdio::inherit())
                .envs(self.ctx.repl_handler().repl_server_env_vars().await?)
                .spawn()?
                .wait()
                .await?;

            if result.code() != Some(75) {
                return result.check_exit_status();
            }

            {
                log_action("Reloading", "TypeScript REPL");
                let _indent = LogIndent::new();
                self.generate_repl_package(&args).await?;
            }
        }
    }

    async fn generate_repl_package(&self, args: &BridgeReplArgs) -> anyhow::Result<()> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let app_ctx = app_ctx.some_or_err_mut()?;

        let package_json_path = args.repl_root_dir.join("package.json");
        let tsconfig_json_path = args.repl_root_dir.join("tsconfig.json");
        let repl_ts_path = args.repl_root_dir.join("repl.ts");

        new_task_up_to_date_check(&BuildContext::new(app_ctx, &BuildConfig::new()))
            .with_task_result_marker(GenerateBridgeReplMarkerHash {
                language: GuestLanguage::TypeScript,
            })?
            .with_sources(|| {
                args.agent_type_names.iter().map(|agent_type_name| {
                    args.repl_root_bridge_sdk_dir
                        .join(bridge_client_directory_name(agent_type_name))
                })
            })
            .with_targets(|| vec![&package_json_path, &tsconfig_json_path, &repl_ts_path])
            .run_or_skip(
                || {
                    log_action("Generating", "TypeScript REPL package");
                    let _indent = LogIndent::new();

                    self.generate_package_json(&args, &package_json_path)?;
                    self.generate_tsconfig_json(&args, &tsconfig_json_path)?;
                    self.generate_repl_ts(&args, &repl_ts_path)?;

                    Ok(())
                },
                || {
                    log_skipping_up_to_date("generating TypeScript REPL package");
                },
            )?;

        Ok(())
    }

    fn generate_package_json(
        &self,
        args: &BridgeReplArgs,
        package_json_path: &Path,
    ) -> anyhow::Result<()> {
        let relative_bridge_sdk_unix_path = args
            .repl_root_bridge_sdk_dir
            .strip_prefix(&args.repl_root_dir)?
            .display()
            .to_string()
            .replace("\\", "/");
        let workspaces = args
            .agent_type_names
            .iter()
            .map(|agent_type_name| {
                format!(
                    "{}/{}",
                    relative_bridge_sdk_unix_path,
                    bridge_client_directory_name(agent_type_name)
                )
            })
            .collect::<Vec<_>>();

        let dependencies = args
            .agent_type_names
            .iter()
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
        args: &BridgeReplArgs,
        tsconfig_json_path: &Path,
    ) -> anyhow::Result<()> {
        let tsconfig_json = json!({
          "compilerOptions": {
            "composite": true,
            "declaration": true,
            "esModuleInterop": true,
            "forceConsistentCasingInFileNames": true,
            "module": "ES2022",
            "moduleResolution": "nodenext",
            "skipLibCheck": true,
            "sourceMap": true,
            "strict": true,
            "target": "ES2022"
          },
          "include": [
            format!("{}/ts/**/*.ts", args.repl_root_bridge_sdk_dir.display())
          ]
        });

        fs::write_str(
            tsconfig_json_path,
            serde_json::to_string_pretty(&tsconfig_json)?,
        )
    }

    fn generate_repl_ts(&self, args: &BridgeReplArgs, repl_ts_path: &Path) -> anyhow::Result<()> {
        let agents_config = args
            .agent_type_names
            .iter()
            .map(|agent_type_name| {
                formatdoc! {"
                    '{agent_type_name}': {{
                      typeName: '{agent_type_name}',
                      clientPackageName: '{client_package_name}',
                      package: await import('{client_package_name}'),
                    }}",
                    client_package_name = bridge_client_directory_name(agent_type_name)
                }
                .lines()
                .enumerate()
                .map(|(idx, l)| {
                    if idx == 0 {
                        l.to_string()
                    } else {
                        format!("    {l}")
                    }
                })
                .join("\n")
            })
            .collect::<Vec<_>>()
            .join(",\n");

        let repl_ts = formatdoc! {"
            import 'tsx/patch-repl';
            const {{ Repl }} = await import('@golem/golem-ts-repl');

            const repl = new Repl({{
              agents: {{
                {agents_config}
              }}
            }});

            await repl.run();
        "};

        fs::write_str(repl_ts_path, repl_ts)
    }
}
