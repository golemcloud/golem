use crate::context::{Context, EnvConfig, K8sNamespace, K8sRoutingType, Runtime};
use anyhow::Result;
use kube::Client;
use std::io;
use std::io::BufRead;
use testcontainers::clients;

mod benchmark;
pub mod cli;
pub mod context;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let docker = clients::Cli::default();
    let mut conf = EnvConfig::from_env();
    conf.runtime = Runtime::K8S {
        namespace: K8sNamespace("benchmark".to_string()),
        routing: K8sRoutingType::Ingress,
    };
    conf.schema = "http".to_string();
    let context = Context::start(&docker, conf).await?;

    for _ in io::stdin().lock().lines() {}

    drop(context);
    drop(docker);

    Ok(())
}
