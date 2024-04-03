use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use libtest_mimic::{Arguments, Conclusion, Failed};
use std::sync::Arc;
use testcontainers::clients;

pub mod cli;
mod template;
mod worker;

fn run(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Conclusion {
    let args = Arguments::from_args();

    let mut tests = Vec::new();

    tests.append(&mut template::all(deps.clone()));
    tests.append(&mut worker::all(deps));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();

    let docker = clients::Cli::default();
    let deps: Arc<dyn TestDependencies + Send + Sync + 'static> =
        Arc::new(EnvBasedTestDependencies::new(&docker));

    let res = run(deps);

    res.exit()
}
