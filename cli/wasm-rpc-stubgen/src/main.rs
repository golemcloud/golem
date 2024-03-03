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
use golem_wasm_rpc_stubgen::*;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    match Command::parse() {
        Command::Generate(generate_args) => {
            let _ = render_error(generate(generate_args));
        }
        Command::Build(build_args) => {
            let _ = render_error(build(build_args).await);
        }
        Command::AddStubDependency(add_stub_dependency_args) => {
            let _ = render_error(add_stub_dependency(add_stub_dependency_args));
        }
        Command::Compose(compose_args) => {
            let _ = render_error(compose(compose_args));
        }
        Command::InitializeWorkspace(init_workspace_args) => {
            let _ = render_error(initialize_workspace(
                init_workspace_args,
                "golem-wasm-rpc-stubgen",
                &[],
            ));
        }
    }
}

fn render_error<T>(result: anyhow::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!("Error: {:?}", err);
            None
        }
    }
}
