pub mod context;
pub mod regular_bootstrap;
pub mod regular_worker_executor;
pub mod worker_ctx;

use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_test_framework::config::TestDependencies;

use crate::regular_mode::context::RegularExecutorTestContext;
use crate::regular_mode::regular_bootstrap::RegularWorkerExecutorBootstrap;
use crate::regular_mode::regular_worker_executor::TestRegularWorkerExecutor;
use crate::{get_golem_config, RegularWorkerExecutorTestDependencies};
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::Bootstrap;
use prometheus::Registry;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::info;

pub async fn start_regular_worker_executor(
    deps: &RegularWorkerExecutorTestDependencies,
    context: &RegularExecutorTestContext,
) -> anyhow::Result<TestRegularWorkerExecutor> {
    let redis = deps.redis();
    let redis_monitor = deps.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();

    let admin_account_id = deps.cloud_service.admin_account_id();

    let prometheus = golem_worker_executor::metrics::register_all();
    let config = get_golem_config(
        redis.public_port(),
        context.redis_prefix(),
        context.grpc_port(),
        context.http_port(),
        admin_account_id,
    );
    let handle = Handle::current();

    let grpc_port = config.port;

    let mut join_set = JoinSet::new();

    run_regular_executor(config, prometheus, handle, &mut join_set).await?;

    let start = std::time::Instant::now();
    loop {
        let client = WorkerExecutorClient::connect(format!("http://127.0.0.1:{grpc_port}")).await;
        if client.is_ok() {
            let deps = deps
                .per_test_dependencies(
                    &context.redis_prefix(),
                    context.http_port(),
                    context.grpc_port(),
                )
                .await;
            break Ok(TestRegularWorkerExecutor::new(Some(join_set), deps));
        } else if start.elapsed().as_secs() > 10 {
            break Err(anyhow::anyhow!("Timeout waiting for server to start"));
        }
    }
}

async fn run_regular_executor(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    info!("Golem Worker Executor starting up...");

    RegularWorkerExecutorBootstrap
        .run(golem_config, prometheus_registry, runtime, join_set)
        .await?;

    Ok(())
}
