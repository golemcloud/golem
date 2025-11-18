pub mod debug_bootstrap;
pub mod debug_worker_executor;
pub mod dsl;

use crate::debug_mode::debug_bootstrap::TestDebuggingServerBootStrap;
use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use golem_common::config::RedisConfig;
use golem_debugging_service::config::DebugConfig;
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::service::compiled_component::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
};
use golem_worker_executor::services::golem_config::{
    AgentTypesServiceConfig, AgentTypesServiceLocalConfig, EngineConfig, IndexedStorageConfig,
    IndexedStorageKVStoreRedisConfig, KeyValueStorageConfig,
};
use golem_worker_executor::Bootstrap;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use prometheus::Registry;
use std::path::Path;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::debug;

pub async fn start_debug_worker_executor(
    // Debug worker executor which is essentially per test needs to know a few details about the regular worker executor
    test_worker_executor: &TestWorkerExecutor,
) -> anyhow::Result<DebugWorkerExecutorClient> {
    let config = DebugConfig {
        key_value_storage: KeyValueStorageConfig::Redis(RedisConfig {
            port: test_worker_executor.deps.redis.public_port(),
            key_prefix: test_worker_executor.context.redis_prefix(),
            ..Default::default()
        }),
        indexed_storage: IndexedStorageConfig::KVStoreRedis(IndexedStorageKVStoreRedisConfig {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: Path::new("data/blobs").to_path_buf(),
        }),
        http_port: 0,
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        agent_types_service: AgentTypesServiceConfig::Local(AgentTypesServiceLocalConfig {}),
        engine: EngineConfig {
            enable_fs_cache: true,
        },
        ..Default::default()
    };

    let handle = Handle::current();

    let mut join_set = JoinSet::new();

    let run_details = run_debug_worker_executor(
        config,
        prometheus::Registry::new(),
        handle,
        &mut join_set,
        test_worker_executor.clone(),
    )
    .await?;

    let start = std::time::Instant::now();

    loop {
        let debug_worker_executor_result = DebugWorkerExecutorClient::connect(
            run_details.http_port,
            test_worker_executor.context.account_token.clone(),
        )
        .await;

        match debug_worker_executor_result {
            Ok(mut client) => {
                client.set_worker_executor_join_set(join_set);
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
    golem_config: DebugConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    regular_executor_context: TestWorkerExecutor,
) -> Result<golem_worker_executor::RunDetails, anyhow::Error> {
    let run_details = TestDebuggingServerBootStrap::new(regular_executor_context)
        .run(
            golem_config.into_golem_config(),
            prometheus_registry,
            runtime,
            join_set,
        )
        .await?;
    Ok(run_details)
}
