use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use golem_test_framework::config::EnvBasedTestDependenciesConfig;
use std::fmt::{Display, Formatter};
use strum_macros::EnumIter;
use test_r::test_dep;
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

test_r::enable!();

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

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::io::stderr)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
async fn test_dependencies(_tracing: &Tracing) -> CloudEnvBasedTestDependencies {
    let deps = CloudEnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        keep_docker_containers: true,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await;

    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    deps
}
