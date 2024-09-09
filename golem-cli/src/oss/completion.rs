use crate::command::profile::OssProfileAdd;
use crate::init::PrintCompletion;
use crate::oss::command::GolemOssCommand;
use clap::CommandFactory;

pub fn print_completion(generator: clap_complete::Shell) {
    let mut cmd = GolemOssCommand::<OssProfileAdd>::command();
    let cmd_name = cmd.get_name().to_string();
    tracing::info!("Golem CLI - generating completion file for {generator:?}...");
    clap_complete::generate(generator, &mut cmd, cmd_name, &mut std::io::stdout());
}

pub struct PrintOssCompletion();

impl PrintCompletion for PrintOssCompletion {
    fn print_completion(&self, generator: clap_complete::Shell) {
        print_completion(generator)
    }
}
