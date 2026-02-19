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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::evcxr_repl::{ReplConfig, REPL_CONFIG_FILE_NAME};
use crate::log::{log_action, logln, set_log_output, LogIndent, Output};
use crate::model::repl::{BridgeReplArgs, ReplScriptSource};
use crate::process::ExitStatusExt;
use crate::{binary_path_to_string, fs, GOLEM_EVCXR_REPL};
use std::sync::Arc;
use tokio::process::Command;

pub struct RustRepl {
    ctx: Arc<Context>,
}

impl RustRepl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn run(&self, args: BridgeReplArgs) -> anyhow::Result<()> {
        {
            log_action("Preparing", "Rust REPL");
            let _indent = LogIndent::new();

            self.generate_repl_config_json(&args).await?;
        }

        logln("");

        loop {
            let result = self.prepare_command(&args).await?.spawn()?.wait().await?;

            if args.script.is_some() {
                set_log_output(Output::TracingDebug);
                return result.pipe_exit_status();
            }

            if result.code() != Some(75) {
                return result.check_exit_status();
            }

            {
                logln("");
                log_action("Reloading", "Rust REPL");
                logln("");
            }
        }
    }

    async fn generate_repl_config_json(&self, args: &BridgeReplArgs) -> anyhow::Result<()> {
        let config_path = args.repl_root_dir.join(REPL_CONFIG_FILE_NAME);
        let config = ReplConfig {
            binary: binary_path_to_string()?,
            app_main_dir: fs::path_to_str(&args.app_main_dir)?.to_string(),
            history_file: fs::path_to_str(&args.repl_history_file_path)?.to_string(),
            cli_commands_metadata_json_path: fs::path_to_str(
                &args.repl_cli_commands_metadata_json_path,
            )?
            .to_string(),
            repl_metadata_json_path: fs::path_to_str(&args.repl_metadata_json_path)?.to_string(),
            golem_client_dependency: self.ctx.template_sdk_overrides().golem_client_dep()?,
        };
        fs::write(&config_path, serde_json::to_string(&config)?)?;
        Ok(())
    }

    async fn prepare_command(&self, args: &BridgeReplArgs) -> anyhow::Result<Command> {
        let mut command = Command::new(binary_path_to_string()?);

        command
            .current_dir(&args.repl_root_dir)
            .env(GOLEM_EVCXR_REPL, "1")
            .envs(self.ctx.repl_handler().repl_server_env_vars().await?)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .stdin(std::process::Stdio::inherit());

        if args.disable_auto_imports {
            command.arg("--disable-auto-imports");
        }
        if !args.stream_logs {
            command.arg("--disable-stream");
        }
        if let Some(script) = &args.script {
            match script {
                ReplScriptSource::Inline(script) => {
                    command.arg("--script");
                    command.arg(script);
                }
                ReplScriptSource::FromFile(path) => {
                    command.arg("--script-file");
                    command.arg(fs::path_to_str(path)?);
                }
            }
        }

        Ok(command)
    }
}
