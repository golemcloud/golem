use crate::components::cloud_service::spawned::SpawnedCloudService;
use crate::components::cloud_service::CloudService;
use crate::components::{CloudEnvVars, TestDependencies};
use golem_test_framework::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::component_service::spawned::SpawnedComponentService;
use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::components::rdb::sqlite::SqliteRdb;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::docker::DockerRedis;
use golem_test_framework::components::redis::provided::ProvidedRedis;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::spawned::SpawnedRedisMonitor;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::spawned::SpawnedShardManager;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::components::worker_service::spawned::SpawnedWorkerService;
use golem_test_framework::components::worker_service::WorkerService;
use golem_test_framework::config::{DbType, EnvBasedTestDependenciesConfig};
use std::path::Path;
use std::sync::Arc;

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
    pub fn blocking_new(config: EnvBasedTestDependenciesConfig) -> Self {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { Self::new(config).await })
    }

    async fn make_rdb(
        config: Arc<EnvBasedTestDependenciesConfig>,
    ) -> Arc<dyn Rdb + Send + Sync + 'static> {
        match config.db_type {
            DbType::Sqlite => {
                let sqlite_path = Path::new("../target/golem_test_db_dir");
                Arc::new(SqliteRdb::new(sqlite_path))
            }
            DbType::Postgres => Arc::new(
                DockerPostgresRdb::new(
                    !config.golem_docker_services,
                    config.keep_docker_containers,
                )
                .await,
            ),
        }
    }

    async fn make_redis(
        config: Arc<EnvBasedTestDependenciesConfig>,
    ) -> Arc<dyn Redis + Send + Sync + 'static> {
        let prefix = config.redis_key_prefix.clone();
        if config.golem_docker_services {
            Arc::new(DockerRedis::new(prefix, config.keep_docker_containers).await)
        } else {
            let host = config.redis_host.clone();
            let port = config.redis_port;
            if golem_test_framework::components::redis::check_if_running(&host, port) {
                Arc::new(ProvidedRedis::new(host, port, prefix))
            } else {
                Arc::new(SpawnedRedis::new(
                    port,
                    prefix,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                ))
            }
        }
    }

    async fn make_redis_monitor(
        config: Arc<EnvBasedTestDependenciesConfig>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        Arc::new(SpawnedRedisMonitor::new(
            redis,
            config.default_stdout_level(),
            config.default_stderr_level(),
        ))
    }

    async fn make_shard_manager(
        config: Arc<EnvBasedTestDependenciesConfig>,
        env_vars: CloudEnvVars,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        if config.golem_docker_services {
            todo!()
        } else {
            Arc::new(
                SpawnedShardManager::new_base(
                    Box::new(env_vars),
                    Path::new("../target/debug/cloud-shard-manager"),
                    Path::new("../cloud-shard-manager"),
                    config.number_of_shards_override,
                    9021,
                    9020,
                    redis,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_component_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        env_vars: CloudEnvVars,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        if config.golem_docker_services {
            todo!()
        } else {
            Arc::new(
                SpawnedComponentService::new_base(
                    config.golem_test_components.clone(),
                    Box::new(env_vars),
                    Path::new("../target/debug/cloud-component-service"),
                    Path::new("../cloud-component-service"),
                    8081,
                    9091,
                    Some(9094),
                    rdb,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                    config.golem_client_protocol,
                )
                .await,
            )
        }
    }

    async fn make_component_compilation_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        env_vars: CloudEnvVars,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        if config.golem_docker_services {
            todo!()
        } else {
            Arc::new(
                SpawnedComponentCompilationService::new_base(
                    Box::new(env_vars),
                    Path::new("../target/debug/cloud-component-compilation-service"),
                    Path::new("../cloud-component-compilation-service"),
                    8083,
                    9094,
                    component_service,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            )
        }
    }

    async fn make_worker_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        env_vars: CloudEnvVars,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        if config.golem_docker_services {
            todo!()
        } else {
            Arc::new(
                SpawnedWorkerService::new_base(
                    Box::new(env_vars),
                    Path::new("../target/debug/cloud-worker-service"),
                    Path::new("../cloud-worker-service"),
                    8082,
                    9092,
                    9093,
                    component_service,
                    shard_manager,
                    rdb,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_verbosity(),
                    config.golem_client_protocol,
                )
                .await,
            )
        }
    }

    async fn make_worker_executor_cluster(
        config: Arc<EnvBasedTestDependenciesConfig>,
        env_vars: CloudEnvVars,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        if config.golem_docker_services {
            todo!()
        } else {
            Arc::new(
                SpawnedWorkerExecutorCluster::new_base(
                    Arc::new(env_vars),
                    config.worker_executor_cluster_size,
                    9000,
                    9100,
                    Path::new("../target/debug/cloud-worker-executor"),
                    Path::new("../cloud-worker-executor"),
                    redis,
                    component_service,
                    shard_manager,
                    worker_service,
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                    false,
                )
                .await,
            )
        }
    }

    async fn make_cloud_service(
        config: Arc<EnvBasedTestDependenciesConfig>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
    ) -> Arc<dyn CloudService + Send + Sync + 'static> {
        if config.golem_docker_services {
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
                    config.default_verbosity(),
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            )
        }
    }

    pub async fn new(config: EnvBasedTestDependenciesConfig) -> Self {
        let config = Arc::new(config);
        let rdb = Self::make_rdb(config.clone()).await;
        let redis = Self::make_redis(config.clone()).await;
        let cloud_service =
            Self::make_cloud_service(config.clone(), rdb.clone(), redis.clone()).await;
        let env_vars = CloudEnvVars {
            cloud_service: cloud_service.clone(),
            redis: redis.clone(),
        };
        let component_service =
            Self::make_component_service(config.clone(), env_vars.clone(), rdb.clone()).await;
        let component_compilation_service = Self::make_component_compilation_service(
            config.clone(),
            env_vars.clone(),
            component_service.clone(),
        )
        .await;
        let redis_monitor = Self::make_redis_monitor(config.clone(), redis.clone()).await;
        let shard_manager =
            Self::make_shard_manager(config.clone(), env_vars.clone(), redis.clone()).await;
        let worker_service = Self::make_worker_service(
            config.clone(),
            env_vars.clone(),
            component_service.clone(),
            shard_manager.clone(),
            rdb.clone(),
        )
        .await;
        let worker_executor_cluster = Self::make_worker_executor_cluster(
            config.clone(),
            env_vars.clone(),
            component_service.clone(),
            shard_manager.clone(),
            worker_service.clone(),
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
