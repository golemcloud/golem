use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use libtest_mimic::{Arguments, Conclusion, Failed};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

pub mod cli;
pub mod component;
pub mod components;
pub mod config;

fn run(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Conclusion {
    let args = Arguments::from_args();

    let mut tests = Vec::new();
    tests.extend(component::all(deps));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let deps: Arc<dyn TestDependencies + Send + Sync + 'static> =
        Arc::new(CloudEnvBasedTestDependencies::blocking_new(3));
    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    let res = run(deps.clone());

    drop(cluster);
    drop(deps);

    res.exit()
}
