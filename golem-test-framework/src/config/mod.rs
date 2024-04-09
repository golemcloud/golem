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

pub use cli::CliTestDependencies;
pub use env::EnvBasedTestDependencies;

use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::shard_manager::ShardManager;
use crate::components::template_service::TemplateService;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;

mod env;
mod cli;

pub trait TestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static>;
    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static>;
    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static>;
    fn template_directory(&self) -> PathBuf;
    fn template_service(&self) -> Arc<dyn TemplateService + Send + Sync + 'static>;
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static>;
}

#[macro_export]
macro_rules! lazy_field {
    ($iface:ident) => {
        Arc<Mutex<Option<Arc<dyn $iface + Send + Sync + 'static>>>>
    }
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}
