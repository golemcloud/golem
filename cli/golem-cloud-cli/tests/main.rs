use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use golem_test_framework::config::EnvBasedTestDependenciesConfig;
use libtest_mimic::{Arguments, Conclusion, Failed};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use strum_macros::EnumIter;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

pub mod account;
pub mod api_definition;
pub mod cli;
pub mod component;
pub mod components;
pub mod config;
pub mod get;
pub mod policy;
pub mod project;
pub mod share;
pub mod token;
pub mod worker;

#[derive(Debug, Copy, Clone, EnumIter)]
pub enum RefKind {
    Name,
    Url,
    Urn,
}

impl Display for RefKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RefKind::Name => write!(f, "name"),
            RefKind::Url => write!(f, "url"),
            RefKind::Urn => write!(f, "urn"),
        }
    }
}

fn run(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Conclusion {
    let args = Arguments::from_args();

    let mut tests = Vec::new();
    tests.extend(component::all(deps.clone()));
    tests.extend(worker::all(deps.clone()));
    tests.extend(api_definition::all(deps.clone()));
    tests.extend(account::all(deps.clone()));
    tests.extend(project::all(deps.clone()));
    tests.extend(token::all(deps.clone()));
    tests.extend(policy::all(deps.clone()));
    tests.extend(share::all(deps.clone()));
    tests.extend(get::all(deps));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let deps: Arc<dyn TestDependencies + Send + Sync + 'static> = Arc::new(
        CloudEnvBasedTestDependencies::blocking_new(EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 3,
            keep_docker_containers: true,
            ..EnvBasedTestDependenciesConfig::new()
        }),
    );
    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    let res = run(deps.clone());

    drop(cluster);
    drop(deps);

    res.exit()
}
