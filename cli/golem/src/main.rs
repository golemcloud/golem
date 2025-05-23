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

use golem::command_handler::ServerCommandHandler;
use golem_cli::command_handler::CommandHandler;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for golem main")
        .block_on(CommandHandler::handle_args(
            std::env::args_os(),
            Arc::new(ServerCommandHandler {}),
        ))
}
