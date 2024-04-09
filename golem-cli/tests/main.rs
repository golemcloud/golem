use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use libtest_mimic::{Arguments, Conclusion, Failed};
use std::sync::Arc;
use tracing::info;

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

    let deps: Arc<dyn TestDependencies + Send + Sync + 'static> =
        Arc::new(EnvBasedTestDependencies::blocking_new(3));
    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    let res = run(deps.clone());

    drop(cluster);
    drop(deps);

    res.exit()
}
