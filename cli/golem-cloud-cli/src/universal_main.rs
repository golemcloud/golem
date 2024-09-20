extern crate derive_more;

use clap::Parser;
use golem_cli::command::profile::UniversalProfileAdd;
use golem_cli::config::{get_config_dir, Config, Profile};
use golem_cli::init::{CliKind, GolemInitCommand};
use golem_cli::oss::command::GolemOssCommand;
use golem_cli::{oss, run_main, ConfiguredMainArgs, InitMainArgs};
use golem_cloud_cli::cloud;
use golem_cloud_cli::cloud::command::GolemCloudCommand;
use std::process::ExitCode;

fn main() -> ExitCode {
    let config_dir = get_config_dir();

    if let Some(profile) = Config::get_active_profile(CliKind::Universal, &config_dir) {
        let cli_kind = CliKind::Universal;
        let profile_name = profile.name.clone();

        match profile.profile {
            Profile::Golem(profile) => run_main(
                oss::main::async_main,
                ConfiguredMainArgs {
                    cli_kind,
                    config_dir,
                    profile_name,
                    profile,
                    command: GolemOssCommand::<UniversalProfileAdd>::parse(),
                },
            ),
            Profile::GolemCloud(profile) => run_main(
                cloud::main::async_main,
                ConfiguredMainArgs {
                    cli_kind,
                    config_dir,
                    profile_name,
                    profile,
                    command: GolemCloudCommand::<UniversalProfileAdd>::parse(),
                },
            ),
        }
    } else {
        run_main(
            golem_cli::init::async_main,
            InitMainArgs {
                cli_kind: CliKind::Universal,
                config_dir,
                command: GolemInitCommand::<UniversalProfileAdd>::parse(),
            },
        )
    }
}
