// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

#![recursion_limit = "512"]

use golem::command_handler::ServerCommandHandler;
use golem_cli::command::GolemCliCommand;
use golem_cli::command_handler::{CommandHandler, requires_executor_runtime};
use golem_cli::main_wrapper;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    if let Some(result) = golem_worker_executor::xfs_quota_helper::run_if_requested() {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err:#}");
                ExitCode::FAILURE
            }
        };
    }

    if let Some(flag) = std::env::args_os()
        .nth(1)
        .and_then(|flag| flag.into_string().ok())
        && flag.starts_with("--dump-config")
    {
        return match golem::config::dump_config(&flag) {
            Ok(_) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Failed to dump config: {err:#}");
                ExitCode::FAILURE
            }
        };
    }

    let command_parse_result = GolemCliCommand::try_parse_from_lenient(std::env::args_os(), true);

    if requires_executor_runtime(&command_parse_result) {
        golem_worker_executor::bootstrap::create_runtime()
            .expect("Failed to build tokio runtime")
            .block_on(CommandHandler::handle(
                command_parse_result,
                Arc::new(ServerCommandHandler {}),
            ))
    } else {
        main_wrapper(|| {
            CommandHandler::handle(command_parse_result, Arc::new(ServerCommandHandler {}))
        })
    }
}
