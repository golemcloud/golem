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

use crate::fs;
use crate::model::cli_command_metadata::CliCommandMetadata;
use crate::model::repl::ReplMetadata;
use anyhow::Context;
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct ReplResolvedConfig {
    pub base_config: ReplConfig,
    pub cli_command_metadata: CliCommandMetadata,
    pub repl_metadata: ReplMetadata,
    pub client_config: ClientConfig,
    pub cli_args: CliArgs,
}

impl ReplResolvedConfig {
    pub fn load() -> anyhow::Result<Self> {
        let cli_args = CliArgs::try_parse_from(std::env::args().skip(1))?;

        let base_config =
            serde_json::from_str::<ReplConfig>(&fs::read_to_string("repl-config.json")?)
                .with_context(|| "Failed to read repl-config.json")?;

        let client_config = ClientConfig::from_env()?;

        let cli_command_metadata = serde_json::from_str::<CliCommandMetadata>(&fs::read_to_string(
            &base_config.cli_commands_metadata_json_path,
        )?)
        .with_context(|| {
            format!(
                "Failed to parse {}",
                base_config.cli_commands_metadata_json_path
            )
        })?;

        let repl_metadata = serde_json::from_str::<ReplMetadata>(&fs::read_to_string(
            &base_config.repl_metadata_json_path,
        )?)
        .with_context(|| format!("Failed to parse {}", base_config.repl_metadata_json_path))?;

        Ok(Self {
            base_config,
            cli_command_metadata,
            repl_metadata,
            client_config,
            cli_args,
        })
    }

    pub fn script_mode(&self) -> bool {
        self.cli_args.script.is_some() || self.cli_args.script_file.is_some()
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

#[derive(Clone, Debug, Parser)]
#[command(disable_help_flag = true, disable_help_subcommand = true)]
pub struct CliArgs {
    #[arg(long)]
    pub script: Option<String>,
    #[arg(long = "script-file")]
    pub script_file: Option<String>,
    #[arg(long = "disable-auto-imports")]
    pub disable_auto_imports: bool,
    #[arg(long = "disable-stream")]
    pub disable_stream: bool,
}

pub const REPL_CONFIG_FILE_NAME: &str = "repl-config.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplConfig {
    pub binary: String,
    pub app_main_dir: String,
    pub history_file: String,
    pub cli_commands_metadata_json_path: String,
    pub repl_metadata_json_path: String,
    pub golem_client_dependency: String,
}

fn required_env_var(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("Missing required environment variable: {name}"))
}
