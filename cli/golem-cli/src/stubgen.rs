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

use crate::model::{GolemError, GolemResult};
use colored::Colorize;
use golem_wasm_rpc_stubgen::Command;

pub async fn handle_stubgen(command: Command) -> Result<GolemResult, GolemError> {
    eprintln!(
        "{}",
        "WARNING: THIS COMMAND IS DEPRECATED AND MIGHT MODIFY SOURCE WIT FILES!".yellow()
    );
    eprintln!(
        "{}",
        format!(
            "\nThe recommended new way to handle wasm-rpc stub generation and linking is the {} command.\n",
            "golem-cli app".bold().underline(),
        ).yellow(),
    );
    let result = match command {
        Command::Generate(args) => golem_wasm_rpc_stubgen::generate(args),
        Command::Build(args) => golem_wasm_rpc_stubgen::build(args).await,
        Command::AddStubDependency(args) => golem_wasm_rpc_stubgen::add_stub_dependency(args),
        Command::Compose(args) => golem_wasm_rpc_stubgen::compose(args).await,
        Command::InitializeWorkspace(args) => {
            golem_wasm_rpc_stubgen::initialize_workspace(args, "golem-cli", &["stubgen"])
        }
    };

    result
        .map_err(|err| GolemError(format!("{err:#}")))
        .map(|_| GolemResult::Str("Done".to_string()))
}
