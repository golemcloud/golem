extern crate derive_more;

use clap::Parser;
use golem_cli::command::profile::CloudProfileAdd;
use golem_cli::config::{get_config_dir, CloudProfile, Config, NamedProfile, Profile, ProfileName};
use golem_cli::init::CliKind;
use golem_cli::{run_main, ConfiguredMainArgs};
use golem_cloud_cli::cloud;
use golem_cloud_cli::cloud::command::GolemCloudCommand;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let config_dir = get_config_dir();
    let cli_kind = CliKind::Cloud;

    let (profile_name, profile) = if let Some(p) = Config::get_active_profile(cli_kind, &config_dir)
    {
        let NamedProfile { name, profile } = p;
        match profile {
            Profile::Golem(_) => make_default_profile(&config_dir),
            Profile::GolemCloud(profile) => (name, profile),
        }
    } else {
        make_default_profile(&config_dir)
    };

    run_main(
        cloud::main::async_main,
        ConfiguredMainArgs {
            cli_kind,
            config_dir,
            profile_name,
            profile,
            command: GolemCloudCommand::<CloudProfileAdd>::parse(),
        },
    )
}

fn make_default_profile(config_dir: &Path) -> (ProfileName, CloudProfile) {
    let name = ProfileName::default(CliKind::Cloud);
    let profile = CloudProfile::default();
    Config::set_profile(
        name.clone(),
        Profile::GolemCloud(profile.clone()),
        config_dir,
    )
    .expect("Failed to create default profile");
    Config::set_active_profile_name(name.clone(), CliKind::Cloud, config_dir)
        .expect("Failed to set active profile");

    (name, profile)
}
