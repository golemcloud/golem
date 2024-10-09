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

use clap::Parser;
use colored::Colorize;
use golem_wasm_rpc_stubgen::*;
use std::process::ExitCode;

#[cfg(test)]
test_r::enable!();

#[tokio::main]
async fn main() -> ExitCode {
    pretty_env_logger::init();

    let result = match Command::parse() {
        Command::Generate(generate_args) => generate(generate_args),
        Command::Build(build_args) => build(build_args).await,
        Command::AddStubDependency(add_stub_dependency_args) => {
            add_stub_dependency(add_stub_dependency_args)
        }
        Command::Compose(compose_args) => compose(compose_args).await,
        Command::InitializeWorkspace(init_workspace_args) => {
            initialize_workspace(init_workspace_args, "wasm-rpc-stubgen", &[])
        }
        #[cfg(feature = "unstable-dec-dep")]
        Command::App { subcommand } => run_declarative_command(subcommand).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{}", format!("Error: {:#}", err).yellow());
            ExitCode::FAILURE
        }
    }
}
