use golem_cli::command::profile::CloudProfileAdd;
use golem_cli::config::{get_config_dir, CloudProfile, Config, NamedProfile, Profile, ProfileName};
use golem_cli::init::CliKind;
use golem_cli::init_tracing;
use golem_cli::model::text::fmt::format_error;
use golem_cli::model::PrintRes;
use golem_cloud_cli::cloud;
use golem_cloud_cli::cloud::cli::GolemCloudCli;
use std::path::Path;
use std::process::ExitCode;
use tracing::info;

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

    let (command, parsed) =
        golem_cli::command::command_and_parsed::<GolemCloudCli<CloudProfileAdd>>();

    let format = parsed.format.unwrap_or(profile.config.default_format);

    init_tracing(parsed.verbosity);

    info!(
        profile = format!("{:?}", profile_name),
        format = format!("{:?}", format),
        "Starting Golem CLI",
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for cli main");

    let result = runtime.block_on(crate::cloud::cli::run(
        config_dir,
        profile_name,
        profile,
        format,
        command,
        parsed,
    ));

    match result {
        Ok(result) => {
            result.println(format);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", format_error(&error.0));
            ExitCode::FAILURE
        }
    }
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
