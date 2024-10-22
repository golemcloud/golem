pub mod cloud_service;
pub mod component_compilation_service;
pub mod component_service;
pub mod rdb;
pub mod shard_manager;
pub mod worker_executor;
pub mod worker_service;

use crate::components::cloud_service::CloudService;
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::RedisMonitor;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_executor_cluster::WorkerExecutorCluster;
use golem_test_framework::components::worker_service::WorkerService;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const ROOT_TOKEN: &str = "2A354594-7A63-4091-A46B-CC58D379F677";

#[derive(Clone)]
pub struct CloudEnvVars {
    pub cloud_service: Arc<dyn CloudService + Send + Sync + 'static>,
    pub redis: Arc<dyn Redis + Send + Sync + 'static>,
}

pub trait TestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static>;
    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static>;
    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static>;
    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static>;
    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static>;
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>;
    fn cloud_service(&self) -> Arc<dyn CloudService + Send + Sync + 'static>;
    fn component_directory(&self) -> PathBuf {
        Path::new("./test-components").to_path_buf()
    }
}
