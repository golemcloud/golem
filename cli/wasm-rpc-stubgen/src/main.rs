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

use clap::{CommandFactory, FromArgMatches};
use colored::Colorize;
use golem_wasm_rpc_stubgen::*;
use std::process::ExitCode;

use golem_wasm_rpc_stubgen::model::app::ComponentPropertiesExtensionsNone;

#[tokio::main]
async fn main() -> ExitCode {
    pretty_env_logger::init();

    let mut clap_command = App::command();

    // Based on Command::parse, but using cloned command, so we avoid creating clap_command twice
    let parsed_command = {
        let mut matches = clap_command.clone().get_matches();
        let res =
            App::from_arg_matches_mut(&mut matches).map_err(|err| err.format(&mut App::command()));
        res.unwrap_or_else(|e| e.exit())
    };

    let result = run_app_command::<ComponentPropertiesExtensionsNone>(
        {
            // TODO: it would be nice to use the same logic which is used by default for handling help,
            //       and that way include the current context (bin name and parent commands),
            //       but that seems to be using errors, error formating and exit directly;
            //       and quite different code path compared to calling print_help
            clap_command
                .find_subcommand_mut("app")
                .unwrap()
                .clone()
                .override_usage(format!(
                    "{} [OPTIONS] [COMMAND]",
                    "wasm-rpc-stubgen app".bold()
                ))
        },
        parsed_command,
    )
    .await;

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", format!("Error: {:#}", err).yellow());
            ExitCode::FAILURE
        }
    }
}
