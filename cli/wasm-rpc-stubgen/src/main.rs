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

mod cargo;
mod rust;
mod stub;
mod wit;

use crate::cargo::generate_cargo_toml;
use crate::rust::generate_stub_source;
use crate::stub::StubDefinition;
use crate::wit::{copy_wit_files, generate_stub_wit};

use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "wasm-rpc-stubgen")]
#[command(bin_name = "wasm-rpc-stubgen")]
enum Command {
    Generate(GenerateArgs),
}

#[derive(clap::Args)]
#[command(version, about, long_about = None)]
struct GenerateArgs {
    #[clap(short, long)]
    source_wit_root: PathBuf,
    #[clap(short, long)]
    dest_crate_root: PathBuf,
    #[clap(short, long)]
    world: Option<String>,
    #[clap(long, default_value = "0.0.1")]
    stub_crate_version: String,
    #[clap(long)]
    wasm_rpc_path_override: Option<String>,
}

fn main() {
    match Command::parse() {
        Command::Generate(generate_args) => {
            let _ = render_error(generate(generate_args));
        }
    }
}

fn render_error<T>(result: anyhow::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!("Error: {}", err);
            None
        }
    }
}

fn generate(args: GenerateArgs) -> anyhow::Result<()> {
    let stub_def = StubDefinition::new(
        &args.source_wit_root,
        &args.dest_crate_root,
        &args.world,
        &args.stub_crate_version,
        &args.wasm_rpc_path_override,
    )
    .context("Failed to gather information for the stub generator")?;

    generate_stub_wit(&stub_def).context("Failed to generate the stub wit file")?;
    copy_wit_files(&stub_def).context("Failed to copy the dependent wit files")?;
    stub_def
        .verify_target_wits()
        .context("Failed to resolve the result WIT root")?;
    generate_cargo_toml(&stub_def).context("Failed to generate the Cargo.toml file")?;
    generate_stub_source(&stub_def).context("Failed to generate the stub Rust source")?;
    Ok(())
}
