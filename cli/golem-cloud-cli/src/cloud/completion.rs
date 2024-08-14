use crate::cloud::command::GolemCloudCommand;
use clap::{Command, CommandFactory};
use golem_cli::command::profile::{CloudProfileAdd, UniversalProfileAdd};

fn print_completion(mut cmd: Command, generator: clap_complete::Shell) {
    let cmd_name = cmd.get_name().to_string();
    tracing::info!("Golem Cloud CLI - generating completion file for {generator:?}...");
    clap_complete::generate(generator, &mut cmd, cmd_name, &mut std::io::stdout());
}

pub fn print_cloud_completion(generator: clap_complete::Shell) {
    print_completion(GolemCloudCommand::<CloudProfileAdd>::command(), generator);
}

pub fn print_universal_completion(generator: clap_complete::Shell) {
    print_completion(
        GolemCloudCommand::<UniversalProfileAdd>::command(),
        generator,
    );
}
