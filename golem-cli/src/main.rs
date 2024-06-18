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

use clap::Parser;
use clap_verbosity_flag::{Level, Verbosity};
use golem_cli::command::profile::OssProfileAdd;
use golem_cli::config::{Config, Profile};
use indoc::formatdoc;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

use golem_cli::init::{CliKind, DummyProfileAuth, GolemInitCommand};
use golem_cli::oss;
use golem_cli::oss::command::GolemOssCommand;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = dirs::home_dir().unwrap();
    let default_conf_dir = home.join(".golem");
    let config_dir = std::env::var("GOLEM_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_conf_dir);

    if let Some(p) = Config::get_active_profile(CliKind::Universal, &config_dir) {
        let name = p.name.clone();

        match p.profile {
            Profile::Golem(p) => {
                let command = GolemOssCommand::<OssProfileAdd>::parse();

                init_tracing(&command.verbosity);
                info!("Golem CLI with profile: {}", name);

                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(oss::main::async_main(command, p, config_dir))
            }
            Profile::GolemCloud(_) => {
                let message = formatdoc!(
                    "Golem Cloud profile is not supported in this CLI version.
                    To use Golem Cloud please install golem-cloud-cli with feature 'universal' to replace golem-cli
                    cargo install --features universal golem-cloud-cli
                    And remove golem-cli:
                    cargo remove golem-cli
                    To create another default profile use `golem-cli init`.
                    To manage profiles use `golem-cli profile` command."
                );

                Err(message)?
            }
        }
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
                CliKind::Universal,
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
