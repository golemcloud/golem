// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::sync::Arc;
use tempfile::TempDir;
use test_r::{tag_suite, test_dep};
use tracing::Level;

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
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
use golem_wasm_ast::analysis::wit_parser::{AnalysedTypeResolve, SharedAnalysedTypeResolve};

mod common;

pub mod api;
pub mod blobstore;
pub mod compatibility;
pub mod durability;
pub mod guest_languages1;
pub mod guest_languages2;
pub mod guest_languages3;
pub mod hot_update;
pub mod indexed_storage;
pub mod key_value_storage;
pub mod keyvalue;
pub mod measure_test_component_mem;
pub mod observability;
pub mod rdbms;
pub mod rdbms_service;
pub mod revert;
pub mod rust_rpc;
pub mod rust_rpc_stubless;
pub mod scalability;
pub mod transactions;
pub mod ts_rpc1_stubless;
pub mod ts_rpc2_stubless;
pub mod wasi;

test_r::enable!();

tag_suite!(api, group1);
tag_suite!(blobstore, group1);
tag_suite!(keyvalue, group1);

tag_suite!(rdbms, group1);

tag_suite!(guest_languages1, group2);

tag_suite!(transactions, group3);
tag_suite!(wasi, group3);
tag_suite!(revert, group3);
tag_suite!(durability, group3);

tag_suite!(scalability, group4);
tag_suite!(hot_update, group4);
tag_suite!(rust_rpc, group4);
tag_suite!(rust_rpc_stubless, group4);

tag_suite!(guest_languages2, group5);

tag_suite!(ts_rpc1, group6);
tag_suite!(ts_rpc1_stubless, group6);

tag_suite!(guest_languages3, group7);

tag_suite!(ts_rpc2, group8);
tag_suite!(ts_rpc2_stubless, group8);

#[derive(Clone)]
pub struct WorkerExecutorPerTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
}

#[async_trait]
impl TestDependencies for WorkerExecutorPerTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn component_temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
    }
}

pub struct WorkerExecutorTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    blob_storage: Arc<dyn BlobStorage + Send + Sync + 'static>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
}

impl Debug for WorkerExecutorTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerExecutorTestDependencies")
    }
}

impl WorkerExecutorTestDependencies {
    pub async fn new() -> Self {
        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(SpawnedRedis::new(
            6379,
            "".to_string(),
            Level::INFO,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::TRACE, Level::ERROR),
        );

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(Path::new("data/blobs"))
                .await
                .unwrap(),
        );
        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let plugin_wasm_files_service = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let component_directory = Path::new("../test-components").to_path_buf();
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
            initial_component_files_service,
            plugin_wasm_files_service,
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
        }
    }

    pub fn per_test(
        &self,
        redis_prefix: &str,
        http_port: u16,
        grpc_port: u16,
    ) -> WorkerExecutorPerTestDependencies {
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
        // Fake worker service forwarding all requests to the worker executor directly
        let worker_service: Arc<dyn WorkerService + 'static> = Arc::new(
            ForwardingWorkerService::new(worker_executor.clone(), self.component_service()),
        );
        WorkerExecutorPerTestDependencies {
            redis,
            redis_monitor: self.redis_monitor.clone(),
            worker_executor,
            worker_service,
            component_service: self.component_service().clone(),
            component_directory: self.component_directory.clone(),
            blob_storage: self.blob_storage.clone(),
            initial_component_files_service: self.initial_component_files_service.clone(),
            plugin_wasm_files_service: self.plugin_wasm_files_service.clone(),
            component_temp_directory: self.component_temp_directory.clone(),
        }
    }
}

#[async_trait]
impl TestDependencies for WorkerExecutorTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn redis(&self) -> Arc<dyn Redis + Send + Sync + 'static> {
        self.redis.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor + Send + Sync + 'static> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn component_service(&self) -> Arc<dyn ComponentService> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + 'static> {
        panic!("Not supported")
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        self.initial_component_files_service.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync + 'static> {
        self.blob_storage.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn component_temp_directory(&self) -> &Path {
        self.component_temp_directory.path()
    }
}

#[derive(Debug)]
pub struct Tracing;

#[test_dep]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("worker-executor-tests").with_env_overrides(),
    );

    Tracing
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> WorkerExecutorTestDependencies {
    WorkerExecutorTestDependencies::new().await
}

#[derive(Debug)]
pub struct LastUniqueId {
    pub id: AtomicU16,
}

#[test_dep]
pub fn last_unique_id() -> LastUniqueId {
    LastUniqueId {
        id: AtomicU16::new(0),
    }
}

#[test_dep(tagged_as = "golem_host")]
pub fn golem_host_analysed_type_resolve() -> SharedAnalysedTypeResolve {
    SharedAnalysedTypeResolve::new(
        AnalysedTypeResolve::from_wit_directory(Path::new("../wit")).unwrap(),
    )
}
