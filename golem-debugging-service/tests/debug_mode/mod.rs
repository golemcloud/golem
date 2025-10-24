pub mod context;
pub mod debug_bootstrap;
pub mod debug_worker_executor;
pub mod dsl;

use golem_test_framework::config::TestDependencies;

use crate::debug_mode::context::DebugExecutorTestContext;
use crate::debug_mode::debug_bootstrap::TestDebuggingServerBootStrap;
use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use crate::{get_golem_config, RegularExecutorTestContext, RegularWorkerExecutorTestDependencies};
use golem_common::model::auth::TokenSecret;
use golem_worker_executor::services::golem_config::GolemConfig;
use golem_worker_executor::Bootstrap;
use prometheus::Registry;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::debug;

pub async fn start_debug_worker_executor(
    // Debug worker executor which is essentially per test needs to know a few details about the regular worker executor
    regular_worker_dependencies: &RegularWorkerExecutorTestDependencies,
    debug_context: &DebugExecutorTestContext,
) -> anyhow::Result<DebugWorkerExecutorClient> {
    let redis = regular_worker_dependencies.redis();
    let redis_monitor = regular_worker_dependencies.redis_monitor();
    redis.assert_valid();
    redis_monitor.assert_valid();
    let prometheus = golem_worker_executor::metrics::register_all();

    let admin_account_id = regular_worker_dependencies.cloud_service.admin_account_id();

    let config = get_golem_config(
        redis.public_port(),
        debug_context.redis_prefix(),
        debug_context.grpc_port(),
        debug_context.http_port(),
        admin_account_id,
    );

    let handle = Handle::current();

    let http_port = config.http_port;

    let mut join_set = JoinSet::new();

    run_debug_worker_executor(
        config,
        prometheus,
        handle,
        &mut join_set,
        debug_context.regular_worker_executor_context(),
    )
    .await?;

    let start = std::time::Instant::now();

    loop {
        let debug_worker_executor_result =
            DebugWorkerExecutorClient::connect(http_port, TokenSecret::new(uuid::Uuid::new_v4()))
                .await;

        match debug_worker_executor_result {
            Ok(client) => {
                break Ok(client);
            }
            Err(e) => {
                debug!("Waiting to connect to debug worker executor: {:?}", e);

                if start.elapsed().as_secs() > 10 {
                    break Err(anyhow::anyhow!("Timeout waiting for server to start"));
                }
            }
        }
    }
}

async fn run_debug_worker_executor(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    regular_executor_context: RegularExecutorTestContext,
) -> Result<(), anyhow::Error> {
    TestDebuggingServerBootStrap::new(regular_executor_context)
        .run(golem_config, prometheus_registry, runtime, join_set)
        .await?;
    Ok(())
}
