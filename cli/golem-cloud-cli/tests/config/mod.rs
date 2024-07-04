use crate::components::cloud_service::spawned::SpawnedCloudService;
use crate::components::cloud_service::CloudService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_service::spawned::SpawnedComponentService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::sqlite::SqliteRdb;
use crate::components::rdb::Rdb;
use crate::components::redis::docker::DockerRedis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis::Redis;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::components::TestDependencies;
use std::path::Path;
use std::sync::Arc;
use tracing::Level;

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}

pub struct CloudEnvBasedTestDependencies {
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    component_compilation_service: Arc<dyn ComponentCompilationService + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>,
    cloud_service: Arc<dyn CloudService + Send + Sync + 'static>,
}

impl CloudEnvBasedTestDependencies {
    pub fn blocking_new(worker_executor_cluster_size: usize) -> Self {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { Self::new(worker_executor_cluster_size).await })
    }

    fn use_docker() -> bool {
        false
    }

    fn db_type() -> DbType {
        let db_type_str = std::env::var("GOLEM_TEST_DB")
            .unwrap_or("".to_string())
            .to_lowercase();
        if db_type_str == "postgres" {
            DbType::Postgres
        } else {
            DbType::Sqlite
        }
    }

    async fn make_rdb() -> Arc<dyn Rdb + Send + Sync + 'static> {
        match Self::db_type() {
            DbType::Sqlite => {
                let sqlite_path = Path::new("../target/golem_test_db_dir");
                Arc::new(SqliteRdb::new(sqlite_path))
            }
            DbType::Postgres => Arc::new(DockerPostgresRdb::new(!Self::use_docker()).await),
        }
    }

    fn is_quiet() -> bool {
        std::env::var("VERBOSE").is_err()
    }

    fn redis_host() -> Option<String> {
        std::env::var("REDIS_HOST").ok()
    }

    fn redis_port() -> Option<u16> {
        std::env::var("REDIS_PORT")
            .ok()
            .map(|port| port.parse().expect("Failed to parse REDIS_PORT"))
    }

    fn redis_prefix() -> Option<String> {
        std::env::var("REDIS_KEY_PREFIX").ok()
    }

    async fn make_redis() -> Arc<dyn Redis + Send + Sync + 'static> {
        let prefix = Self::redis_prefix().unwrap_or("".to_string());
        if Self::use_docker() {
            Arc::new(DockerRedis::new(prefix).await)
        } else {
            let host = Self::redis_host().unwrap_or("localhost".to_string());
            let port = Self::redis_port().unwrap_or(6379);

            if crate::components::redis::check_if_running(&host, port) {
                Arc::new(ProvidedRedis::new(host, port, prefix))
            } else {
                Arc::new(SpawnedRedis::new(
                    port,
                    prefix,
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                ))
            }
        }
    }

    async fn make_redis_monitor(
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        Arc::new(SpawnedRedisMonitor::new(
            redis,
            Self::default_stdout_level(),
            Self::default_stderr_level(),
        ))
    }

    async fn make_shard_manager(
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedShardManager::new(
                    Path::new("../target/debug/cloud-shard-manager"),
                    Path::new("../cloud-shard-manager"),
                    9021,
                    9020,
                    redis,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_component_service(
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        cloud: Arc<dyn CloudService + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedComponentService::new(
                    Path::new("../target/debug/cloud-component-service"),
                    Path::new("../cloud-component-service"),
                    8081,
                    9091,
                    Some(9094),
                    rdb,
                    cloud,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_component_compilation_service(
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedComponentCompilationService::new(
                    Path::new("../target/debug/cloud-component-compilation-service"),
                    Path::new("../cloud-component-compilation-service"),
                    8083,
                    9094,
                    component_service,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_worker_service(
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        cloud: Arc<dyn CloudService + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedWorkerService::new(
                    Path::new("../target/debug/cloud-worker-service"),
                    Path::new("../cloud-worker-service"),
                    8082,
                    9092,
                    9093,
                    component_service,
                    shard_manager,
                    cloud,
                    rdb,
                    redis,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_worker_executor_cluster(
        worker_executor_cluster_size: usize,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        cloud_service: Arc<dyn CloudService + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedWorkerExecutorCluster::new(
                    worker_executor_cluster_size,
                    9000,
                    9100,
                    Path::new("../target/debug/cloud-worker-executor"),
                    Path::new("../cloud-worker-executor"),
                    redis,
                    component_service,
                    shard_manager,
                    worker_service,
                    cloud_service,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_cloud_service(
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn CloudService + Send + Sync + 'static> {
        if Self::use_docker() {
            todo!()
        } else {
            Arc::new(
                SpawnedCloudService::new(
                    Path::new("../target/debug/cloud-service"),
                    Path::new("../cloud-service"),
                    8085,
                    9095,
                    redis,
                    rdb,
                    Self::default_verbosity(),
                    Self::default_stdout_level(),
                    Self::default_stderr_level(),
                )
                .await,
            )
        }
    }

    fn default_stdout_level() -> Level {
        if Self::is_quiet() {
            Level::TRACE
        } else {
            Level::INFO
        }
    }

    fn default_stderr_level() -> Level {
        if Self::is_quiet() {
            Level::TRACE
        } else {
            Level::ERROR
        }
    }

    fn default_verbosity() -> Level {
        if Self::is_quiet() {
            Level::INFO
        } else {
            Level::DEBUG
        }
    }

    pub async fn new(worker_executor_cluster_size: usize) -> Self {
        let rdb = Self::make_rdb().await;
        let redis = Self::make_redis().await;
        let cloud_service = Self::make_cloud_service(rdb.clone(), redis.clone()).await;
        let component_service =
            Self::make_component_service(rdb.clone(), cloud_service.clone()).await;
        let component_compilation_service =
            Self::make_component_compilation_service(component_service.clone()).await;
        let redis_monitor = Self::make_redis_monitor(redis.clone()).await;
        let shard_manager = Self::make_shard_manager(redis.clone()).await;
        let worker_service = Self::make_worker_service(
            component_service.clone(),
            shard_manager.clone(),
            cloud_service.clone(),
            rdb.clone(),
            redis.clone(),
        )
        .await;
        let worker_executor_cluster = Self::make_worker_executor_cluster(
            worker_executor_cluster_size,
            component_service.clone(),
            shard_manager.clone(),
            worker_service.clone(),
            cloud_service.clone(),
            redis.clone(),
        )
        .await;

        Self {
            rdb,
            redis,
            redis_monitor,
            shard_manager,
            component_service,
            component_compilation_service,
            worker_service,
            worker_executor_cluster,
            cloud_service,
        }
    }
}

impl TestDependencies for CloudEnvBasedTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        self.rdb.clone()
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        self.shard_manager.clone()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        self.component_compilation_service.clone()
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        self.worker_executor_cluster.clone()
    }

    fn cloud_service(&self) -> Arc<dyn CloudService + Send + Sync + 'static> {
        self.cloud_service.clone()
    }
}
