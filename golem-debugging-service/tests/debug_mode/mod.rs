pub mod debug_bootstrap;
pub mod debug_worker_executor;
pub mod dsl;

use crate::debug_mode::debug_bootstrap::TestDebuggingServerBootStrap;
use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use golem_debugging_service::bootstrap_and_run_debug_worker_executor;
use golem_debugging_service::config::DebugConfig;
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::service::compiled_component::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
};
use golem_worker_executor::services::golem_config::{
    AgentTypesServiceConfig, AgentTypesServiceLocalConfig, EngineConfig, IndexedStorageConfig,
    IndexedStorageKVStoreSqliteConfig, KeyValueStorageConfig,
};
use golem_worker_executor_test_utils::{TestWorkerExecutor, sqlite_storage_config};
use prometheus::Registry;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use tracing::debug;

pub async fn start_debug_worker_executor(
    // Debug worker executor which is essentially per test needs to know a few details about the regular worker executor
    test_worker_executor: &TestWorkerExecutor,
) -> anyhow::Result<DebugWorkerExecutorClient> {
    let config = DebugConfig {
        key_value_storage: KeyValueStorageConfig::Sqlite(sqlite_storage_config(
            &test_worker_executor.deps,
            &test_worker_executor.context,
        )),
        indexed_storage: IndexedStorageConfig::KVStoreSqlite(IndexedStorageKVStoreSqliteConfig {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: test_worker_executor.deps.blob_storage_root(),
        }),
        http_port: 0,
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        agent_types_service: AgentTypesServiceConfig::Local(AgentTypesServiceLocalConfig {}),
        engine: EngineConfig {
            enable_fs_cache: true,
        },
        cors_origin_regex: ".*".to_string(),
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
                client.set_worker_executor_join_set_and_run_details(join_set, run_details);
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
) -> Result<golem_debugging_service::RunDetails, anyhow::Error> {
    let cors_origin_regex = golem_config.cors_origin_regex.clone();
    let run_details = bootstrap_and_run_debug_worker_executor(
        &TestDebuggingServerBootStrap::new(regular_executor_context),
        golem_config.into_golem_config(),
        &cors_origin_regex,
        prometheus_registry,
        runtime,
        join_set,
    )
    .await?;
    Ok(run_details)
}
