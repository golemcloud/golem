// Copyright 2024-2025 Golem Cloud
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

use crate::components::component_compilation_service::ComponentCompilationService;
use async_trait::async_trait;
pub use cli::{CliParams, CliTestDependencies, CliTestService};
pub use env::EnvBasedTestDependencies;
pub use env::EnvBasedTestDependenciesConfig;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::path::PathBuf;
use std::sync::Arc;

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::service::Service;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;

pub mod cli;
mod env;

#[async_trait]
pub trait TestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static>;
    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static>;
    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static>;
    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static>;
    fn component_directory(&self) -> PathBuf;
    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static>;
    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static>;
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>;

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService>;

    async fn kill_all(&self) {
        self.worker_executor_cluster().kill_all().await;
        self.worker_service().kill().await;
        self.component_compilation_service().kill().await;
        self.component_service().kill().await;
        self.shard_manager().kill().await;
        self.rdb().kill().await;
        self.redis_monitor().kill();
        self.redis().kill().await;
    }
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}

pub trait TestService {
    fn service(&self) -> Arc<dyn Service + Send + Sync + 'static>;

    fn kill_all(&self) {
        self.service().kill();
    }
}
