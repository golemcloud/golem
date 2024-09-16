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
use golem_cli::command::profile::OssProfileAdd;
use golem_cli::config::{get_config_dir, Config, NamedProfile, Profile};
use golem_cli::init::{CliKind, GolemInitCommand};
use golem_cli::oss::command::GolemOssCommand;
use golem_cli::{oss, run_main, ConfiguredMainArgs, InitMainArgs};
use indoc::eprintdoc;
use std::process::ExitCode;

fn main() -> ExitCode {
    let config_dir = get_config_dir();
    let cli_kind = CliKind::Oss;

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

    if let Some((profile_name, profile)) = oss_profile {
        run_main(
            oss::main::async_main,
            ConfiguredMainArgs {
                cli_kind,
                config_dir,
                profile_name,
                profile,
                command: GolemOssCommand::<OssProfileAdd>::parse(),
            },
        )
    } else {
        run_main(
            golem_cli::init::async_main,
            InitMainArgs {
                cli_kind,
                config_dir,
                command: GolemInitCommand::<OssProfileAdd>::parse(),
            },
        )
    }
}
