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

use clap::Parser;
use launch::{launch_golem_services, LaunchArgs};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Option<SubCommand>,
}

#[derive(Parser, Debug)]
enum SubCommand {
    #[clap(name = "start", about = "Start a golem server for local development")]
    Start {
        /// Port to listen on
        #[clap(short, long, default_value_t = 8080)]
        port: u16,

        /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
        #[clap(short, long)]
        data_dir: Option<PathBuf>,

        /// Verbose mode
        #[clap(short, long, default_value_t = false)]
        verbose: bool,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();

    let base_directories =
        xdg::BaseDirectories::with_prefix("golem").expect("Failed to get XDG base directories");

    match args.command {
        Some(SubCommand::Start {
            port,
            verbose,
            data_dir,
        }) => {
            let data_dir = data_dir.unwrap_or_else(|| base_directories.get_state_home());

            let result = launch_golem_services(&LaunchArgs {
                port,
                verbose,
                data_dir,
            });

            match result {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("Error: {:?}", e);
                    ExitCode::FAILURE
                }
            }
        }
        None => golem_cli::run_main(),
    }
}
