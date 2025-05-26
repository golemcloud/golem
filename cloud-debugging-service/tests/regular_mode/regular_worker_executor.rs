use async_trait::async_trait;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::config::TestDependencies;

use crate::RegularWorkerExecutorPerTestDependencies;
use tokio::task::JoinSet;

use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::path::Path;
use std::sync::Arc;

pub struct TestRegularWorkerExecutor {
    _join_set: Option<JoinSet<anyhow::Result<()>>>,
    deps: RegularWorkerExecutorPerTestDependencies,
}

impl TestRegularWorkerExecutor {
    pub fn new(
        joins_set: Option<JoinSet<anyhow::Result<()>>>,
        deps: RegularWorkerExecutorPerTestDependencies,
    ) -> Self {
        Self {
            _join_set: joins_set,
            deps,
        }
    }
}

impl Clone for TestRegularWorkerExecutor {
    fn clone(&self) -> Self {
        Self {
            _join_set: None,
            deps: self.deps.clone(),
        }
    }
}

#[async_trait]
impl TestDependencies for TestRegularWorkerExecutor {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        self.deps.rdb()
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.deps.redis()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.deps.blob_storage()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.deps.redis_monitor()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        self.deps.shard_manager()
    }

    fn component_directory(&self) -> &Path {
        self.deps.component_directory()
    }

    fn component_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::component_service::ComponentService + 'static>
    {
        self.deps.component_service()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        self.deps.component_compilation_service()
    }

    fn worker_service(
        &self,
    ) -> Arc<dyn golem_test_framework::components::worker_service::WorkerService + 'static> {
        self.deps.worker_service()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        self.deps.worker_executor_cluster()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.deps.initial_component_files_service()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.deps.plugin_wasm_files_service()
    }

    fn component_temp_directory(&self) -> &Path {
        self.deps.component_temp_directory()
    }
}
