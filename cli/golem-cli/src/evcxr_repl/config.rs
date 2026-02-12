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

use crate::model::cli_command_metadata::CliCommandMetadata;
use clap::Parser;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplConfig {
    pub prompt: String,
    pub history_file: String,
    pub cli_commands_metadata_json_path: Option<String>,
    pub cli_command_metadata: Option<CliCommandMetadata>,
    pub client_config: ClientConfig,
    pub script: Option<String>,
    pub script_path: Option<String>,
    pub disable_auto_imports: bool,
}

impl ReplConfig {
    pub fn load() -> anyhow::Result<Self> {
        let mut config = Self::load_base_config()?;
        config.load_process_args()?;

        if let Some(client_config) = ClientConfig::from_env()? {
            config.client_config = Some(client_config);
        }

        if let Some(path) = config.cli_commands_metadata_json_path.as_ref() {
            let path = std::env::current_dir()?.join(path);
            let contents = std::fs::read(&path)?;
            config.cli_command_metadata = Some(serde_json::from_slice(&contents)?);
        }

        Ok(config)
    }

    pub fn history_path(&self) -> anyhow::Result<Option<PathBuf>> {
        if self.history_file.is_empty() {
            return Ok(None);
        }

        let base_dir = dirs::data_dir().or_else(dirs::config_dir);
        let Some(base_dir) = base_dir else {
            return Ok(None);
        };

        let history_dir = base_dir.join("golem");
        std::fs::create_dir_all(&history_dir)?;
        Ok(Some(history_dir.join(&self.history_file)))
    }

    pub fn prompt_string(&self) -> String {
        if self.script.is_some() {
            return String::new();
        }

        if let Some(client_config) = self.client_config.as_ref() {
            let name = "golem-rust-repl".cyan();
            let app = format!("[{}]", client_config.application.green().bold());
            let env = format!("[{}]", client_config.environment.yellow().bold());
            let arrow = ">".green().bold();
            return format!("{name}{app}{env}{arrow} ");
        }

        self.prompt.clone()
    }

    fn load_base_config() -> anyhow::Result<Self> {
        let path = std::env::current_dir()?.join("repl-config.json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(err.into()),
        };

        Ok(serde_json::from_slice(&bytes)?)
    }

    fn load_process_args(&mut self) -> anyhow::Result<()> {
        let mut args = vec!["repl".to_string()];
        for arg in std::env::args().skip(1) {
            if arg == "-script-file" {
                args.push("--script-file".to_string());
            } else {
                args.push(arg);
            }
        }

        let parsed = CliArgs::try_parse_from(args)?;
        self.disable_auto_imports = parsed.disable_auto_imports;

        if let Some(script) = parsed.script {
            self.script = Some(script);
            self.script_path = None;
            return Ok(());
        }

        if let Some(script_path) = parsed.script_file {
            let script = std::fs::read_to_string(&script_path)?;
            self.script = Some(script);
            self.script_path = Some(script_path);
            return Ok(());
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    pub server: GolemServer,
    pub application: String,
    pub environment: String,
}

impl ClientConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            server: GolemServer::from_env()?,
            application: required_env_var("GOLEM_REPL_APPLICATION")?,
            environment: required_env_var("GOLEM_REPL_ENVIRONMENT")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum GolemServer {
    Local,
    Cloud { token: String },
    Custom { url: String, token: String },
}

impl GolemServer {
    fn from_env() -> anyhow::Result<Self> {
        let server_kind = required_env_var("GOLEM_REPL_SERVER_KIND")?;
        match server_kind.as_str() {
            "local" => Ok(Self::Local),
            "cloud" => Ok(Self::Cloud {
                token: required_env_var("GOLEM_REPL_SERVER_TOKEN")?,
            }),
            "custom" => Ok(Self::Custom {
                url: required_env_var("GOLEM_REPL_SERVER_CUSTOM_URL")?,
                token: required_env_var("GOLEM_REPL_SERVER_TOKEN")?,
            }),
            _ => Err(anyhow::anyhow!(
                "Invalid GOLEM_REPL_SERVER_KIND: {server_kind}"
            )),
        }
    }
}

#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_help_subcommand = true)]
struct CliArgs {
    #[arg(long)]
    script: Option<String>,
    #[arg(long = "script-file")]
    script_file: Option<String>,
    #[arg(long = "disable-auto-imports", default_value_t = false)]
    disable_auto_imports: bool,
}

fn required_env_var(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("Missing required environment variable: {name}"))
}

pub struct CliCommandsConfig {
    pub binary: String,
    pub app_main_dir: PathBuf,
    pub client_config: ClientConfig,
    pub command_metadata: CliCommandMetadata,
}
