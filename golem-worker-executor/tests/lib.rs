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

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
// use golem_test_framework::components::cloud_service::{AdminOnlyStubCloudService, CloudService};
// use golem_test_framework::components::component_service::filesystem::FileSystemComponentService;
// use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::redis::spawned::SpawnedRedis;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::redis_monitor::spawned::SpawnedRedisMonitor;
use golem_test_framework::components::redis_monitor::RedisMonitor;
// use golem_test_framework::components::worker_executor::provided::ProvidedWorkerExecutor;
// use golem_test_framework::components::worker_executor::WorkerExecutor;
// use golem_test_framework::components::worker_service::forwarding::ForwardingWorkerService;
// use golem_test_framework::components::worker_service::WorkerService;
use self::common::component_writer::FileSystemComponentWriter;
use golem_wasm::analysis::wit_parser::{AnalysedTypeResolve, SharedAnalysedTypeResolve};
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::sync::Arc;
use tempfile::TempDir;
use test_r::test_dep;
use tracing::Level;

mod common;

pub mod agent;
pub mod api;
pub mod blobstore;
pub mod compatibility;
pub mod durability;
pub mod hot_update;
// pub mod http;
// pub mod indexed_storage;
// pub mod key_value_storage;
// pub mod keyvalue;
// pub mod observability;
// pub mod rdbms;
// pub mod rdbms_service;
// pub mod revert;
// pub mod rust_rpc;
// pub mod rust_rpc_stubless;
// pub mod scalability;
// pub mod transactions;
// pub mod wasi;

test_r::enable!();

// tag_suite!(api, group1);
// tag_suite!(blobstore, group1);
// tag_suite!(keyvalue, group1);
// tag_suite!(http, group1);
// tag_suite!(rdbms, group1);
// tag_suite!(agent, group1);

// tag_suite!(transactions, group2);
// tag_suite!(wasi, group2);
// tag_suite!(revert, group2);
// tag_suite!(durability, group2);
// tag_suite!(observability, group2);
// tag_suite!(scalability, group2);
// tag_suite!(hot_update, group2);
// tag_suite!(rust_rpc, group2);
// tag_suite!(rust_rpc_stubless, group2);

// tag_suite!(rdbms_service, rdbms_service);

#[derive(Clone)]
pub struct WorkerExecutorTestDependencies {
    redis: Arc<dyn Redis>,
    redis_monitor: Arc<dyn RedisMonitor>,
    component_writer: Arc<FileSystemComponentWriter>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    component_directory: PathBuf,
    component_temp_directory: Arc<TempDir>,
    component_service_directory: PathBuf,
}

impl Debug for WorkerExecutorTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerExecutorTestDependencies")
    }
}

impl WorkerExecutorTestDependencies {
    pub async fn new() -> Self {
        let redis: Arc<dyn Redis> = Arc::new(SpawnedRedis::new(
            6379,
            "".to_string(),
            Level::INFO,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(SpawnedRedisMonitor::new(
            redis.clone(),
            Level::TRACE,
            Level::ERROR,
        ));

        let blob_storage = Arc::new(
            FileSystemBlobStorage::new(Path::new("data/blobs"))
                .await
                .unwrap(),
        );

        let initial_component_files_service =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let component_directory = Path::new("../test-components").to_path_buf();
        let component_service_directory = Path::new("data/components");

        let component_writer: Arc<FileSystemComponentWriter> =
            Arc::new(FileSystemComponentWriter::new(component_service_directory).await);

        Self {
            redis,
            redis_monitor,
            component_directory,
            component_service_directory: component_service_directory.to_path_buf(),
            component_writer,
            initial_component_files_service,
            component_temp_directory: Arc::new(TempDir::new().unwrap()),
        }
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
