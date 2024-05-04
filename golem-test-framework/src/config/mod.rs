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

use std::path::PathBuf;
use std::sync::Arc;

use crate::components::component_compilation_service::ComponentCompilationService;
pub use cli::{CliParams, CliTestDependencies};
pub use env::EnvBasedTestDependencies;

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::service::Service;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;

mod cli;
mod env;

pub trait TestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static>;
    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static>;
    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static>;
    fn component_directory(&self) -> PathBuf;
    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static>;
    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static>;
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>;

    fn kill_all(&self) {
        self.worker_executor_cluster().kill_all();
        self.worker_service().kill();
        self.component_service().kill();
        self.shard_manager().kill();
        self.rdb().kill();
        self.redis_monitor().kill();
        self.redis().kill();
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
