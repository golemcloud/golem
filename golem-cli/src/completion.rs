use clap::Command;

pub fn print_completion(mut command: Command, shell: clap_complete::Shell) {
    let cmd_name = command.get_name().to_string();
    tracing::info!("Golem CLI - generating completion file for {cmd_name} - {shell:?}...");
    clap_complete::generate(shell, &mut command, cmd_name, &mut std::io::stdout());
}

pub trait PrintCompletion {
    fn print_completion(shell: clap_complete::Shell);
}
