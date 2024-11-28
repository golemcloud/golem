// Copyright 2024 Golem Cloud
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

mod launch;
mod migration;
mod proxy;

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use golem_cli::{
    command::CliCommand,
    model::{GolemError, GolemResult},
};
use launch::{launch_golem_services, LaunchArgs};

#[derive(Parser, Debug)]
enum ExtraCommands {
    #[clap(name = "start", about = "Start a golem server for local development")]
    Start {
        /// Port to listen on
        #[clap(short, long, default_value_t = 9881)]
        port: u16,

        /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
        #[clap(short, long)]
        data_dir: Option<PathBuf>,
    },
}

impl<Ctx> CliCommand<Ctx> for ExtraCommands {
    async fn run(self, _ctx: Ctx) -> Result<GolemResult, GolemError> {
        match self {
            ExtraCommands::Start { port, data_dir } => {
                let base_directories = xdg::BaseDirectories::with_prefix("golem")
                    .expect("Failed to get XDG base directories");
                let data_dir = data_dir.unwrap_or_else(|| base_directories.get_state_home());

                match launch_golem_services(&LaunchArgs { port, data_dir }).await {
                    Ok(_) => Ok(GolemResult::Str("".to_string())),
                    Err(e) => Err(GolemError(format!("{e:#}"))),
                }
            }
        }
    }
}

fn main() -> ExitCode {
    golem_cli::run_main::<ExtraCommands>()
}
