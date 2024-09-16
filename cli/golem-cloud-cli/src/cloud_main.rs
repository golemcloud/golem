extern crate derive_more;

use clap::Parser;
use golem_cli::command::profile::CloudProfileAdd;
use golem_cli::config::{CloudProfile, Config, NamedProfile, Profile, ProfileName};
use std::path::{Path, PathBuf};
use tracing::info;

use golem_cli::init::CliKind;
use golem_cli::init_tracing;
use golem_cloud_cli::cloud;
use golem_cloud_cli::cloud::command::GolemCloudCommand;
use golem_cloud_cli::cloud::completion::PrintCloudCompletion;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = dirs::home_dir().unwrap();
    let default_conf_dir = home.join(".golem");
    let config_dir = std::env::var("GOLEM_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_conf_dir);

    let (name, cloud_profile) =
        if let Some(p) = Config::get_active_profile(CliKind::Cloud, &config_dir) {
            let NamedProfile { name, profile } = p;
            match profile {
                Profile::Golem(_) => make_default_profile(&config_dir),
                Profile::GolemCloud(profile) => (name, profile),
            }
        } else {
            make_default_profile(&config_dir)
        };

    let command = GolemCloudCommand::<CloudProfileAdd>::parse();

    init_tracing(&command.verbosity);
    info!("Golem Cloud CLI with profile: {}", name);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(cloud::main::async_main(
            command,
            name,
            cloud_profile,
            CliKind::Cloud,
            config_dir,
            Box::new(PrintCloudCompletion()),
        ))
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
