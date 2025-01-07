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

use crate::launch::{launch_golem_services, LaunchArgs};
use clap::Parser;
use golem_cli::{
    command::CliCommand,
    model::{GolemError, GolemResult},
};
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub enum SingleExecutableCommand {
    #[clap(name = "start", about = "Start a golem server for local development")]
    Start {
        /// Address to serve the main API on
        #[clap(long, default_value = "0.0.0.0")]
        router_addr: String,

        /// Port to serve the main API on
        #[clap(long, default_value_t = 9881)]
        router_port: u16,

        /// Port to serve custom requests on
        #[clap(long, default_value_t = 9006)]
        custom_request_port: u16,

        /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
        #[clap(long)]
        data_dir: Option<PathBuf>,

        /// Clean the data directory before starting
        #[clap(long, default_value = "false")]
        clean: bool,
    },
}

impl<Ctx> CliCommand<Ctx> for SingleExecutableCommand {
    async fn run(self, _ctx: Ctx) -> Result<GolemResult, GolemError> {
        match self {
            SingleExecutableCommand::Start {
                router_addr: router_host,
                router_port,
                custom_request_port,
                data_dir,
                clean,
            } => {
                let base_directories = xdg::BaseDirectories::with_prefix("golem")
                    .map_err(|_| GolemError("Failed to get XDG base directories".to_string()))?;

                let data_dir = data_dir.unwrap_or_else(|| base_directories.get_state_home());

                if clean && tokio::fs::metadata(&data_dir).await.is_ok() {
                    tokio::fs::remove_dir_all(&data_dir)
                        .await
                        .map_err(|e| GolemError(format!("Failed cleaning data dir: {e:#}")))?;
                };

                match launch_golem_services(&LaunchArgs {
                    router_host,
                    router_port,
                    custom_request_port,
                    data_dir,
                })
                .await
                {
                    Ok(_) => Ok(GolemResult::Str("".to_string())),
                    Err(e) => Err(GolemError(format!("{e:#}"))),
                }
            }
        }
    }
}
