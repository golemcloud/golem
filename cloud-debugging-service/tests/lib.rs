use async_trait::async_trait;
use golem_common::config::RedisConfig;
use golem_common::model::RetryConfig;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::component_service::filesystem::FileSystemComponentService;
use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::provided::ProvidedRedis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::spawned::SpawnedRedisMonitor;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor::provided::ProvidedWorkerExecutor;
use golem_test_framework::components::worker_executor::WorkerExecutor;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::components::worker_service::forwarding::ForwardingWorkerService;
use golem_test_framework::components::worker_service::WorkerService;
use golem_test_framework::config::TestDependencies;
use golem_worker_executor::services::golem_config::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig, ComponentCacheConfig,
    ComponentServiceConfig, ComponentServiceLocalConfig, GolemConfig, IndexedStorageConfig,
    IndexedStorageKVStoreRedisConfig, KeyValueStorageConfig, MemoryConfig,
    ShardManagerServiceConfig, ShardManagerServiceSingleShardConfig, WorkerServiceGrpcConfig,
};
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_r::test_dep;
use tracing::Level;

pub mod debug_mode;
pub mod debug_tests;
pub mod regular_mode;
pub mod services;

pub use debug_mode::context::DebugExecutorTestContext;
pub use debug_mode::dsl::TestDslDebugMode;
pub use debug_mode::start_debug_worker_executor;

pub use regular_mode::context::RegularExecutorTestContext;
pub use regular_mode::start_regular_worker_executor;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

#[derive(Debug)]
pub struct LastUniqueId {
    pub id: AtomicU16,
}

// Dependencies
#[test_dep]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(&TracingConfig::test_pretty_without_time(
        "debugging-executor-tests",
    ));

    Tracing
}

#[test_dep]
pub fn last_unique_id() -> LastUniqueId {
    LastUniqueId {
        id: AtomicU16::new(0),
    }
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> RegularWorkerExecutorTestDependencies {
    RegularWorkerExecutorTestDependencies::new().await
}

pub fn get_golem_config(
    redis_public_port: u16,
    redis_prefix: String,
    server_port: u16, // This can be grpc port for regular worker executor or http port for debug worker executor
    http_port: u16,
) -> GolemConfig {
    GolemConfig {
        key_value_storage: KeyValueStorageConfig::Redis(RedisConfig {
            port: redis_public_port,
            key_prefix: redis_prefix,
            ..Default::default()
        }),
        indexed_storage: IndexedStorageConfig::KVStoreRedis(IndexedStorageKVStoreRedisConfig {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: Path::new("data/blobs").to_path_buf(),
        }),
        port: server_port,
        http_port,
        compiled_component_service: CompiledComponentServiceConfig::Enabled(
            CompiledComponentServiceEnabledConfig {},
        ),
        shard_manager_service: ShardManagerServiceConfig::SingleShard(
            ShardManagerServiceSingleShardConfig {},
        ),
        public_worker_api: WorkerServiceGrpcConfig {
            host: "localhost".to_string(),
            port: server_port,
            access_token: "03494299-B515-4427-8C37-4C1C915679B7".to_string(),
            retries: RetryConfig::max_attempts_5(),
            connect_timeout: Duration::from_secs(120),
        },
        memory: MemoryConfig::default(),
        ..Default::default()
    }
}

pub fn get_component_service_config() -> ComponentServiceConfig {
    ComponentServiceConfig::Local(ComponentServiceLocalConfig {
        root: PathBuf::from("data/components"),
    })
}

pub fn get_component_cache_config() -> ComponentCacheConfig {
    ComponentCacheConfig::default()
}

// In a debugging test suite, we have a regular worker executor with its own dependencies
#[derive(Clone)]
pub struct RegularWorkerExecutorPerTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    _worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
}

#[async_trait]
impl TestDependencies for RegularWorkerExecutorPerTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("rdb not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("Shard manager is not supported in debugging tests. We directly place things in a running worker executor")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("compilation service supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Debugging executor tests do not support worker executor clusters")
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }
}

pub struct RegularWorkerExecutorTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
}

impl Debug for RegularWorkerExecutorTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerExecutorTestDependencies")
    }
}

impl RegularWorkerExecutorTestDependencies {
    pub async fn new() -> Self {
        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(SpawnedRedis::new(
            6379,
            "".to_string(),
            Level::INFO,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(Path::new("data/blobs"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));
        let plugin_wasm_files_service = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let component_directory =
            Path::new("../cloud-debugging-service/test-components").to_path_buf();
        let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
            FileSystemComponentService::new(
                Path::new("data/components"),
                plugin_wasm_files_service.clone(),
            )
            .await,
        );

        Self {
            redis,
            redis_monitor,
            component_directory,
            component_service,
            blob_storage,
            plugin_wasm_files_service,
            initial_component_files_service,
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
        }
    }

    pub async fn per_test_dependencies(
        &self,
        redis_prefix: &str,
        http_port: u16,
        grpc_port: u16,
    ) -> RegularWorkerExecutorPerTestDependencies {
        // Connecting to the primary Redis but using a unique prefix
        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(ProvidedRedis::new(
            self.redis.public_host().to_string(),
            self.redis.public_port(),
            redis_prefix.to_string(),
        ));
        // Connecting to the worker executor started in-process
        let worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static> = Arc::new(
            ProvidedWorkerExecutor::new("localhost".to_string(), http_port, grpc_port, true),
        );

        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            ForwardingWorkerService::new(worker_executor.clone(), self.component_service()),
        );

        RegularWorkerExecutorPerTestDependencies {
            redis,
            redis_monitor: self.redis_monitor.clone(),
            _worker_executor: worker_executor,
            worker_service,
            component_service: self.component_service().clone(),
            component_directory: self.component_directory.clone(),
            blob_storage: self.blob_storage.clone(),
            initial_component_files_service: self.initial_component_files_service.clone(),
            plugin_wasm_files_service: self.plugin_wasm_files_service.clone(),
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
        }
    }
}

#[async_trait]
impl TestDependencies for RegularWorkerExecutorTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("rdb test dependency Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("shard manager dependency supported")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("component compilation service not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + 'static> {
        panic!("worker service dependency not supported")
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("worker executor cluster supported")
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }
}
