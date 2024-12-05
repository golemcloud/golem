use golem_cli::command::profile::UniversalProfileAdd;
use golem_cli::command::EmptyCommand;
use golem_cli::config::{get_config_dir, Config, Profile};
use golem_cli::init::CliKind;
use golem_cli::init_tracing;
use golem_cli::model::text::fmt::format_error;
use golem_cli::model::{Format, GolemError, GolemResult};
use golem_cli::oss::cli::GolemOssCli;
use golem_cloud_cli::cloud;
use golem_cloud_cli::cloud::cli::GolemCloudCli;
use std::process::ExitCode;

fn main() -> ExitCode {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for cli main");

    runtime.block_on(async_main())
}

// TODO: this is really not great as the schema for the command is different depending on whether we are in oss or cloud mode.
// This messes with completions that users generate and use in their shell.
// Better would be to nest the oss commands under a subcommand.
async fn async_main() -> ExitCode {
    let config_dir = get_config_dir();
    let cli_kind = CliKind::Universal;

    if let Some(profile) = Config::get_active_profile(CliKind::Universal, &config_dir) {
        let profile_name = profile.name.clone();

        match profile.profile {
            Profile::Golem(profile) => {
                let (command, parsed) = golem_cli::command::command_and_parsed::<
                    GolemOssCli<UniversalProfileAdd, EmptyCommand>,
                >();

                let format = parsed.format.unwrap_or(profile.config.default_format);

                init_tracing(parsed.verbosity.clone());

                // TODO: We probably want to use some auth here. The oss cli uses a DummyProfileAuth here.
                let result = golem_cli::oss::cli::run_with_profile(
                    format, config_dir, profile, command, parsed, cli_kind,
                )
                .await;
                result_to_exit_code(result, format)
            }
            Profile::GolemCloud(profile) => {
                let (command, parsed) =
                    golem_cli::command::command_and_parsed::<GolemCloudCli<UniversalProfileAdd>>();

                let format = parsed.format.unwrap_or(profile.config.default_format);

                init_tracing(parsed.verbosity.clone());

                let result = crate::cloud::cli::run(
                    config_dir,
                    profile_name,
                    profile,
                    format,
                    command,
                    parsed,
                )
                .await;
                result_to_exit_code(result, format)
            }
        }
    } else {
        let (command, parsed) = golem_cli::command::command_and_parsed::<
            GolemOssCli<UniversalProfileAdd, EmptyCommand>,
        >();

        let format = parsed.format.unwrap_or_default();

        init_tracing(parsed.verbosity.clone());

        // TODO: We probably want to use some auth here. The oss cli uses a DummyProfileAuth here.
        let result =
            golem_cli::oss::cli::run_without_profile(config_dir, command, parsed, cli_kind).await;
        result_to_exit_code(result, format)
    }
}

fn result_to_exit_code(result: Result<GolemResult, GolemError>, format: Format) -> ExitCode {
    match result {
        Ok(result) => {
            result.print(format);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", format_error(&error.0));
            ExitCode::FAILURE
        }
    }
}
