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

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ctor::{ctor, dtor};
use golem_test_framework::components::component_compilation_service::ComponentCompilationService;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

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

mod common;

pub mod api;
pub mod blob_storage;
pub mod blobstore;
pub mod guest_languages;
pub mod guest_languages2;
pub mod hot_update;
pub mod keyvalue;
pub mod measure_test_component_mem;
pub mod rpc;
pub mod scalability;
pub mod transactions;
pub mod wasi;

#[derive(Clone)]
pub(crate) struct WorkerExecutorPerTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    component_directory: PathBuf,
}

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

    fn component_directory(&self) -> PathBuf {
        self.component_directory.clone()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }
}

struct WorkerExecutorTestDependencies {
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    component_directory: PathBuf,
}

impl WorkerExecutorTestDependencies {
    pub fn new() -> Self {
        let redis: Arc<dyn Redis + Send + Sync + 'static> = Arc::new(SpawnedRedis::new(
            6379,
            "".to_string(),
            Level::INFO,
            Level::ERROR,
        ));
        let redis_monitor: Arc<dyn RedisMonitor + Send + Sync + 'static> = Arc::new(
            SpawnedRedisMonitor::new(redis.clone(), Level::DEBUG, Level::ERROR),
        );
        let component_directory = Path::new("../test-components").to_path_buf();
        let component_service: Arc<dyn ComponentService + Send + Sync + 'static> = Arc::new(
            FileSystemComponentService::new(Path::new("data/components")),
        );
        Self {
            redis,
            redis_monitor,
            component_directory,
            component_service,
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
            ProvidedWorkerExecutor::new("localhost".to_string(), http_port, grpc_port),
        );
        // Fake worker service forwarding all requests to the worker executor directly
        let worker_service: Arc<dyn WorkerService + Send + Sync + 'static> = Arc::new(
            ForwardingWorkerService::new(worker_executor.clone(), self.component_service()),
        );
        WorkerExecutorPerTestDependencies {
            redis,
            redis_monitor: self.redis_monitor.clone(),
            worker_executor,
            worker_service,
            component_service: self.component_service().clone(),
            component_directory: self.component_directory.clone(),
        }
    }
}

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

    fn component_directory(&self) -> PathBuf {
        self.component_directory.clone()
    }

    fn component_service(&self) -> Arc<dyn ComponentService + Send + Sync + 'static> {
        self.component_service.clone()
    }

    fn component_compilation_service(
        &self,
    ) -> Arc<dyn ComponentCompilationService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync + 'static> {
        panic!("Not supported")
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster + Send + Sync + 'static> {
        panic!("Not supported")
    }
}

#[ctor]
pub static BASE_DEPS: WorkerExecutorTestDependencies = WorkerExecutorTestDependencies::new();

#[dtor]
unsafe fn drop_base_deps() {
    let base_deps_ptr = BASE_DEPS.deref() as *const WorkerExecutorTestDependencies;
    let base_deps_ptr = base_deps_ptr as *mut WorkerExecutorTestDependencies;
    (*base_deps_ptr).redis().kill();
    (*base_deps_ptr).redis_monitor().kill();
}

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        // let console_layer = console_subscriber::spawn().with_filter(
        //     EnvFilter::try_new("trace").unwrap()
        //);
        let ansi_layer = tracing_subscriber::fmt::layer()
            .event_format(tracing_subscriber::fmt::format().without_time().pretty())
            .with_ansi(true)
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive("debug".parse().unwrap())
                    .from_env_lossy()
                    .add_directive("cranelift_codegen=warn".parse().unwrap())
                    .add_directive("wasmtime_cranelift=warn".parse().unwrap())
                    .add_directive("wasmtime_jit=warn".parse().unwrap())
                    .add_directive("h2=warn".parse().unwrap())
                    .add_directive("hyper=warn".parse().unwrap())
                    .add_directive("tower=warn".parse().unwrap())
                    .add_directive("fred=warn".parse().unwrap()),
            );

        tracing_subscriber::registry()
            // .with(console_layer) // Uncomment this to use tokio-console. Also needs RUSTFLAGS="--cfg tokio_unstable"
            .with(ansi_layer)
            .init();

        Self
    }
}

#[ctor]
pub static TRACING: Tracing = Tracing::init();
