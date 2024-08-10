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

extern crate derive_more;

use clap::{CommandFactory, Parser};
use clap_verbosity_flag::{Level, Verbosity};
use golem_cli::command::profile::OssProfileAdd;
use golem_cli::completion::print_completions;
use golem_cli::config::{Config, NamedProfile, Profile};
use golem_cli::init::{CliKind, DummyProfileAuth, GolemInitCommand};
use golem_cli::oss;
use golem_cli::oss::command::{GolemOssCommand, OssCommand};
use indoc::eprintdoc;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = dirs::home_dir().unwrap();
    let default_conf_dir = home.join(".golem");
    let config_dir = std::env::var("GOLEM_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_conf_dir);

    let cli_kind = CliKind::Golem;

    let oss_profile = match Config::get_active_profile(cli_kind, &config_dir) {
        Some(NamedProfile {
            name,
            profile: Profile::Golem(p),
        }) => Some((name, p)),
        Some(NamedProfile {
            name: _,
            profile: Profile::GolemCloud(_),
        }) => {
            eprintdoc!(
                    "Golem Cloud profile is not supported in this CLI version.
                    To use Golem Cloud please install golem-cloud-cli with feature 'universal' to replace golem-cli
                    cargo install --features universal golem-cloud-cli
                    And remove golem-cli:
                    cargo remove golem-cli
                    To create another default profile use `golem-cli init`.
                    To manage profiles use `golem-cli profile` command.
                    "
                );

            None
        }
        None => None,
    };

    // use clap_complete::{generate, Generator, Shell};
    //
    // fn build_cli() -> clap::Command {
    //
    //     GolemOssCommand::<OssProfileAdd>::command().subcommand(
    //         clap::Command::new("completion")
    //             .arg(
    //                 clap::Arg::new("generator")
    //                     .long("generate")
    //                     .value_parser(clap::value_parser!(Shell)),
    //             ))
    //
    // }
    //
    // fn print_completions<G: Generator>(gen: G, cmd: &mut clap::Command) {
    //     generate(gen, cmd, cmd.get_name().to_string(), &mut std::io::stdout());
    // }
    //
    // let matches = build_cli().get_matches();
    //
    // let completion_matches = matches.subcommand().filter(|(c, _)| *c == "completion").map(|(_, m)| m);
    // let generate = completion_matches.and_then(|m| m.try_get_one::<Shell>("generator").ok().flatten());
    //
    //
    // if let Some(generator) = generate {
    //     let mut cmd = build_cli();
    //     eprintln!("Generating completion file for {generator}...");
    //     print_completions(*generator, &mut cmd);
    //     return Ok(());
    // }

    let command = GolemOssCommand::<OssProfileAdd>::parse();

    if let OssCommand::Completion { generator } = command.command {
        let mut command = GolemOssCommand::<OssProfileAdd>::command();
        eprintln!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut command);
        return Ok(());
    }

    if let Some((name, p)) = oss_profile {
        let command = GolemOssCommand::<OssProfileAdd>::parse();
        init_tracing(&command.verbosity);
        info!("Golem CLI with profile: {}", name);

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(oss::main::async_main(command, p, cli_kind, config_dir))
    } else {
        let command = GolemInitCommand::<OssProfileAdd>::parse();

        init_tracing(&command.verbosity);
        info!("Golem Init CLI");

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(golem_cli::init::async_main(
                command,
                cli_kind,
                config_dir,
                Box::new(DummyProfileAuth {}),
            ))
    }
}

fn init_tracing(verbosity: &Verbosity) {
    if let Some(level) = verbosity.log_level() {
        let tracing_level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };

        let subscriber = FmtSubscriber::builder()
            .with_max_level(tracing_level)
            .with_writer(std::io::stderr)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }
}
