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

use std::fs::File;
use std::path::PathBuf;

use clap::{Args, Parser};

use golem_openapi_client_generator::{gen, parse_openapi_specs};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, rename_all = "kebab-case")]
enum Cli {
    /// Generate a client from an OpenAPI spec
    Generate(GenerateArgs),
    /// Merge multiple OpenAPI specs into a single one
    Merge(MergeArgs),
}

#[derive(Debug, Args)]
struct GenerateArgs {
    #[arg(short, long, value_name = "spec", value_hint = clap::ValueHint::FilePath, num_args = 1.., required = true)]
    spec_yaml: Vec<PathBuf>,

    #[arg(short, long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    output_directory: PathBuf,

    #[arg(short = 'v', long, default_value = "0.0.0")]
    client_version: String,

    #[arg(short, long)]
    name: String,
}

#[derive(Debug, Args)]
struct MergeArgs {
    #[arg(short, long, value_name = "specs", value_hint = clap::ValueHint::FilePath, num_args = 1.., required = true)]
    spec_yaml: Vec<PathBuf>,
    #[arg(short, long, value_name = "output", value_hint = clap::ValueHint::FilePath)]
    output_yaml: PathBuf,
}

fn main() {
    let command = Cli::parse();

    match command {
        Cli::Generate(args) => {
            let openapi_specs = parse_openapi_specs(&args.spec_yaml).unwrap();
            gen(
                openapi_specs,
                &args.output_directory,
                &args.name,
                &args.client_version,
                true,
                false,
                &[],
                &[],
            )
            .unwrap();
        }
        Cli::Merge(args) => {
            let openapi_specs = parse_openapi_specs(&args.spec_yaml).unwrap();
            let openapi =
                golem_openapi_client_generator::merge_all_openapi_specs(openapi_specs).unwrap();
            let file = File::create(&args.output_yaml).unwrap();
            serde_yaml::to_writer(file, &openapi).unwrap();
        }
    }
}
