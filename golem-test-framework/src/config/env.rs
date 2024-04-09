// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::Level;
use crate::components;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::Rdb;
use crate::components::rdb::sqlite::SqliteRdb;
use crate::components::redis::docker::DockerRedis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::Redis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::shard_manager::docker::DockerShardManager;
use crate::components::shard_manager::ShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::template_service::docker::DockerTemplateService;
use crate::components::template_service::spawned::SpawnedTemplateService;
use crate::components::template_service::TemplateService;
use crate::components::worker_executor_cluster::docker::DockerWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::docker::DockerWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::components::worker_service::WorkerService;
use crate::config::{DbType, TestDependencies};

pub struct EnvBasedTestDependencies {
    worker_executor_cluster_size: usize,
    rdb: crate::lazy_field!(Rdb),
    redis: crate::lazy_field!(Redis),
    redis_monitor: crate::lazy_field!(RedisMonitor),
    shard_manager: crate::lazy_field!(ShardManager),
    template_service: crate::lazy_field!(TemplateService),
    worker_service: crate::lazy_field!(WorkerService),
    worker_executor_cluster: crate::lazy_field!(WorkerExecutorCluster),
}

impl EnvBasedTestDependencies {
    pub fn new(worker_executor_cluster_size: usize) -> Self {
        Self {
            worker_executor_cluster_size,
            rdb: Arc::new(Mutex::new(None)),
            redis: Arc::new(Mutex::new(None)),
            redis_monitor: Arc::new(Mutex::new(None)),
            shard_manager: Arc::new(Mutex::new(None)),
            template_service: Arc::new(Mutex::new(None)),
            worker_service: Arc::new(Mutex::new(None)),
            worker_executor_cluster: Arc::new(Mutex::new(None)),
        }
    }

    pub fn kill_all(&self) {
        self.worker_executor_cluster().kill_all();
        self.worker_service().kill();
        self.template_service().kill();
        self.shard_manager().kill();
        self.rdb().kill();
        self.redis_monitor().kill();
        self.redis().kill();
    }

    fn db_type() -> DbType {
        let db_type_str = std::env::var("GOLEM_TEST_DB")
            .unwrap_or("".to_string())
            .to_lowercase();
        if db_type_str == "sqlite" {
            DbType::Sqlite
        } else {
            DbType::Postgres
        }
    }

    fn is_quiet() -> bool {
        std::env::var("QUIET").is_ok()
    }

    fn use_docker() -> bool {
        std::env::var("GOLEM_DOCKER_SERVICES").is_ok()
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

    fn golem_test_templates() -> Option<PathBuf> {
        std::env::var("GOLEM_TEST_TEMPLATES")
            .ok()
            .map(|s| Path::new(&s).to_path_buf())
    }
}

impl TestDependencies for EnvBasedTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        let mut rdb_cell = self.rdb.lock().unwrap();
        match &*rdb_cell {
            None => match Self::db_type() {
                DbType::Sqlite => {
                    let sqlite_path = Path::new("../target/golem_test_db");
                    let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                        Arc::new(SqliteRdb::new(sqlite_path));
                    *rdb_cell = Some(rdb.clone());
                    rdb
                }
                DbType::Postgres => {
                    let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
                        Arc::new(DockerPostgresRdb::new(!Self::use_docker()));
                    *rdb_cell = Some(rdb.clone());
                    rdb
                }
            },
            Some(rdb) => rdb.clone(),
        }
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        let mut redis_cell = self.redis.lock().unwrap();
        match &*redis_cell {
            None => {
                let prefix = Self::redis_prefix().unwrap_or("".to_string());
                let redis: Arc<dyn Redis + Send + Sync + 'static> = if Self::use_docker() {
                    Arc::new(DockerRedis::new(prefix))
                } else {
                    let host = Self::redis_host().unwrap_or("localhost".to_string());
                    let port = Self::redis_port().unwrap_or(6379);

                    if components::redis::check_if_running(&host, port) {
                        Arc::new(ProvidedRedis::new(host, port, prefix))
                    } else {
                        Arc::new(SpawnedRedis::new(
                            port,
                            prefix,
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        ))
                    }
                };
                *redis_cell = Some(redis.clone());
                redis
            }
            Some(redis) => redis.clone(),
        }
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        let mut redis_monitor_cell = self.redis_monitor.lock().unwrap();
        match &*redis_monitor_cell {
            None => {
                let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> =
                    Arc::new(SpawnedRedisMonitor::new(
                        self.redis(),
                        Self::default_stdout_level(),
                        Self::default_stderr_level(),
                    ));
                *redis_monitor_cell = Some(redis_monitor.clone());
                redis_monitor
            }
            Some(redis_monitor) => redis_monitor.clone(),
        }
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        let mut shard_manager_cell = self.shard_manager.lock().unwrap();
        match &*shard_manager_cell {
            None => {
                let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> =
                    if Self::use_docker() {
                        Arc::new(DockerShardManager::new(
                            self.redis(),
                            Self::default_verbosity(),
                        ))
                    } else {
                        Arc::new(SpawnedShardManager::new(
                            Path::new("../target/debug/golem-shard-manager"),
                            Path::new("../golem-shard-manager"),
                            9021,
                            9020,
                            self.redis(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        ))
                    };
                *shard_manager_cell = Some(shard_manager.clone());
                shard_manager
            }
            Some(shard_manager) => shard_manager.clone(),
        }
    }

    fn template_directory(&self) -> PathBuf {
        Self::golem_test_templates().unwrap_or(Path::new("../test-templates").to_path_buf())
    }

    fn template_service(&self) -> Arc<dyn TemplateService + Send + Sync + 'static> {
        let mut template_service_cell = self.template_service.lock().unwrap();
        match &*template_service_cell {
            None => {
                let template_service: Arc<dyn TemplateService + Send + Sync + 'static> =
                    if Self::use_docker() {
                        Arc::new(DockerTemplateService::new(
                            self.rdb(),
                            Self::default_verbosity(),
                        ))
                    } else {
                        Arc::new(SpawnedTemplateService::new(
                            Path::new("../target/debug/golem-template-service"),
                            Path::new("../golem-template-service"),
                            8081,
                            9091,
                            self.rdb(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        ))
                    };
                *template_service_cell = Some(template_service.clone());
                template_service
            }
            Some(template_service) => template_service.clone(),
        }
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        let mut worker_service_cell = self.worker_service.lock().unwrap();
        match &*worker_service_cell {
            None => {
                let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> =
                    if Self::use_docker() {
                        Arc::new(DockerWorkerService::new(
                            self.template_service(),
                            self.shard_manager(),
                            self.rdb(),
                            self.redis(),
                            Self::default_verbosity(),
                        ))
                    } else {
                        Arc::new(SpawnedWorkerService::new(
                            Path::new("../target/debug/golem-worker-service"),
                            Path::new("../golem-worker-service"),
                            8082,
                            9092,
                            9093,
                            self.template_service(),
                            self.shard_manager(),
                            self.rdb(),
                            self.redis(),
                            Self::default_verbosity(),
                            Self::default_stdout_level(),
                            Self::default_stderr_level(),
                        ))
                    };
                *worker_service_cell = Some(worker_service.clone());
                worker_service
            }
            Some(worker_service) => worker_service.clone(),
        }
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        let mut worker_executor_cluster_cell = self.worker_executor_cluster.lock().unwrap();
        match &*worker_executor_cluster_cell {
            None => {
                let worker_executor_cluster: Arc<
                    dyn WorkerExecutorCluster + Send + Sync + 'static,
                > = if Self::use_docker() {
                    Arc::new(DockerWorkerExecutorCluster::new(
                        self.worker_executor_cluster_size,
                        9000,
                        9100,
                        self.redis(),
                        self.template_service(),
                        self.shard_manager(),
                        self.worker_service(),
                        Self::default_verbosity(),
                    ))
                } else {
                    Arc::new(SpawnedWorkerExecutorCluster::new(
                        self.worker_executor_cluster_size,
                        9000,
                        9100,
                        Path::new("../target/debug/worker-executor"),
                        Path::new("../golem-worker-executor"),
                        self.redis(),
                        self.template_service(),
                        self.shard_manager(),
                        self.worker_service(),
                        Self::default_verbosity(),
                        Self::default_stdout_level(),
                        Self::default_stderr_level(),
                    ))
                };
                *worker_executor_cluster_cell = Some(worker_executor_cluster.clone());
                worker_executor_cluster
            }
            Some(worker_executor_cluster) => worker_executor_cluster.clone(),
        }
    }
}
