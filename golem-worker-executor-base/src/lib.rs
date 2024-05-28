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

pub mod durable_host;
pub mod error;
pub mod grpc;
pub mod http_server;
pub mod invocation;
pub mod metrics;
pub mod model;
pub mod preview2;
pub mod services;
pub mod storage;
pub mod wasi_host;
pub mod worker;
pub mod workerctx;

use anyhow::anyhow;
use async_trait::async_trait;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_server::WorkerExecutorServer;
use golem_common::redis::RedisPool;
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tonic::transport::Server;
use tracing::info;
use uuid::Uuid;
use wasmtime::component::Linker;
use wasmtime::{Config, Engine};

use crate::grpc::WorkerExecutorImpl;
use crate::http_server::HttpServerImpl;
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
use crate::services::component::ComponentService;
use crate::services::events::Events;
use crate::services::golem_config::{
    BlobStorageConfig, GolemConfig, IndexedStorageConfig, KeyValueStorageConfig,
};
use crate::services::key_value::{DefaultKeyValueService, KeyValueService};
use crate::services::oplog::{OplogService, PrimaryOplogService};
use crate::services::promise::{DefaultPromiseService, PromiseService};
use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};
use crate::services::shard::{ShardService, ShardServiceDefault};
use crate::services::shard_manager::ShardManagerService;
use crate::services::worker::{DefaultWorkerService, WorkerService};
use crate::services::worker_activator::{LazyWorkerActivator, WorkerActivator};
use crate::services::worker_enumeration::{
    DefaultWorkerEnumerationService, RunningWorkerEnumerationService,
    RunningWorkerEnumerationServiceDefault, WorkerEnumerationService,
};
use crate::services::worker_proxy::{RemoteWorkerProxy, WorkerProxy};
use crate::services::{component, shard_manager, All};
use crate::storage::blob::s3::S3BlobStorage;
use crate::storage::blob::BlobStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::IndexedStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::redis::RedisKeyValueStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::workerctx::WorkerCtx;

/// The Bootstrap trait should be implemented by all Worker Executors to customize the initialization
/// of its services.
/// With a valid `Bootstrap` implementation the service can be started with the `run` method.
#[async_trait]
pub trait Bootstrap<Ctx: WorkerCtx> {
    /// Allows customizing the `ActiveWorkers` service.
    fn create_active_workers(&self, golem_config: &GolemConfig) -> Arc<ActiveWorkers<Ctx>>;

    /// Allows customizing the `All` service.
    /// This is the place to initialize additional services and store them in `All`'s `extra_deps`
    /// field.
    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Ctx>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
    ) -> anyhow::Result<All<Ctx>>;

    /// Can be overridden to customize the wasmtime configuration
    fn create_wasmtime_config(&self) -> Config {
        let mut config = Config::default();

        config.wasm_multi_value(true);
        config.async_support(true);
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        config.consume_fuel(true);

        config
    }

    /// This method is responsible for linking all the host function implementations the worker
    /// executor supports.
    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Ctx>>;

    /// Runs the worker executor
    async fn run(
        &self,
        golem_config: GolemConfig,
        prometheus_registry: Registry,
        runtime: Handle,
    ) -> anyhow::Result<()> {
        info!("Golem Worker Executor starting up...");

        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter
            .set_serving::<WorkerExecutorServer<WorkerExecutorImpl<Ctx, All<Ctx>>>>()
            .await;

        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
            .build()
            .unwrap();

        let http_server = HttpServerImpl::new(
            golem_config.http_addr()?,
            prometheus_registry,
            "Worker executor is running",
        );

        let (redis, key_value_storage): (
            Option<RedisPool>,
            Arc<dyn KeyValueStorage + Send + Sync>,
        ) = match &golem_config.key_value_storage {
            KeyValueStorageConfig::Redis(redis) => {
                info!("Using Redis for key-value storage at {}", redis.url());
                let pool = RedisPool::configured(redis)
                    .await
                    .map_err(|err| anyhow!(err))?;
                let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
                    Arc::new(RedisKeyValueStorage::new(pool.clone()));
                (Some(pool), key_value_storage)
            }
            KeyValueStorageConfig::InMemory => {
                info!("Using in-memory key-value storage");
                (None, Arc::new(InMemoryKeyValueStorage::new()))
            }
        };

        let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = match &golem_config
            .indexed_storage
        {
            IndexedStorageConfig::KVStoreRedis => {
                info!("Using the same Redis for indexed-storage");
                let redis = redis
                    .expect("Redis must be configured key-value storage when using KVStoreRedis");
                Arc::new(RedisIndexedStorage::new(redis.clone()))
            }
            IndexedStorageConfig::Redis(redis) => {
                info!("Using Redis for indexed-storage at {}", redis.url());
                let pool = RedisPool::configured(redis).await?;
                Arc::new(RedisIndexedStorage::new(pool.clone()))
            }
            IndexedStorageConfig::InMemory => {
                info!("Using in-memory indexed storage");
                Arc::new(storage::indexed::memory::InMemoryIndexedStorage::new())
            }
        };
        let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &golem_config.blob_storage {
            BlobStorageConfig::S3(config) => {
                info!("Using S3 for blob storage");
                Arc::new(S3BlobStorage::new(config.clone()).await)
            }
            BlobStorageConfig::LocalFileSystem(config) => {
                info!(
                    "Using local file system for blob storage at {:?}",
                    config.root
                );
                Arc::new(
                    storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                        .await
                        .map_err(|err| anyhow!(err))?,
                )
            }
            BlobStorageConfig::InMemory => {
                info!("Using in-memory blob storage");
                Arc::new(storage::blob::memory::InMemoryBlobStorage::new())
            }
        };

        let component_service = component::configured(
            &golem_config.component_service,
            &golem_config.component_cache,
            &golem_config.compiled_component_service,
            blob_storage.clone(),
        )
        .await;

        let golem_config = Arc::new(golem_config.clone());
        let promise_service: Arc<dyn PromiseService + Send + Sync> =
            Arc::new(DefaultPromiseService::new(key_value_storage.clone()));
        let shard_service = Arc::new(ShardServiceDefault::new());
        let lazy_worker_activator = Arc::new(LazyWorkerActivator::new());

        let oplog_service = Arc::new(
            PrimaryOplogService::new(
                indexed_storage.clone(),
                blob_storage.clone(),
                golem_config.oplog.max_operations_before_commit,
                golem_config.oplog.max_payload_size,
            )
            .await,
        );

        let worker_service = Arc::new(DefaultWorkerService::new(
            key_value_storage.clone(),
            shard_service.clone(),
            oplog_service.clone(),
        ));
        let worker_enumeration_service = Arc::new(DefaultWorkerEnumerationService::new(
            worker_service.clone(),
            oplog_service.clone(),
            golem_config.clone(),
        ));

        let active_workers = self.create_active_workers(&golem_config);

        let running_worker_enumeration_service = Arc::new(
            RunningWorkerEnumerationServiceDefault::new(active_workers.clone()),
        );

        let shard_manager_service = shard_manager::configured(&golem_config.shard_manager_service);

        let config = self.create_wasmtime_config();
        let engine = Arc::new(Engine::new(&config)?);
        let linker = self.create_wasmtime_linker(&engine)?;

        let mut epoch_interval = tokio::time::interval(golem_config.limits.epoch_interval);
        let engine_ref: Arc<Engine> = engine.clone();
        tokio::spawn(async move {
            loop {
                epoch_interval.tick().await;
                engine_ref.increment_epoch();
            }
        });

        let linker = Arc::new(linker);

        let key_value_service = Arc::new(DefaultKeyValueService::new(key_value_storage.clone()));

        let blob_store_service = Arc::new(DefaultBlobStoreService::new(blob_storage.clone()));

        let scheduler_service = SchedulerServiceDefault::new(
            key_value_storage.clone(),
            shard_service.clone(),
            promise_service.clone(),
            lazy_worker_activator.clone(),
            golem_config.scheduler.refresh_interval,
        );

        let worker_proxy: Arc<dyn WorkerProxy + Send + Sync> = Arc::new(RemoteWorkerProxy::new(
            golem_config.public_worker_api.uri(),
            golem_config
                .public_worker_api
                .access_token
                .parse::<Uuid>()
                .expect("Access token must be an UUID"),
        ));

        let events = Arc::new(Events::new());

        let services = self
            .create_services(
                active_workers,
                engine,
                linker,
                runtime.clone(),
                component_service,
                shard_manager_service,
                worker_service,
                worker_enumeration_service,
                running_worker_enumeration_service,
                promise_service,
                golem_config.clone(),
                shard_service,
                key_value_service,
                blob_store_service,
                lazy_worker_activator.clone(),
                oplog_service,
                scheduler_service,
                worker_proxy,
                events,
            )
            .await?;

        let addr = golem_config.grpc_addr()?;
        let worker_executor =
            WorkerExecutorImpl::<Ctx, All<Ctx>>::new(services, lazy_worker_activator, addr.port())
                .await?;

        let service = WorkerExecutorServer::new(worker_executor);

        info!("Starting gRPC server on port {}", addr.port());
        Server::builder()
            .concurrency_limit_per_connection(golem_config.limits.concurrency_limit_per_connection)
            .max_concurrent_streams(Some(golem_config.limits.max_concurrent_streams))
            .add_service(reflection_service)
            .add_service(service)
            .add_service(health_service)
            .serve(addr)
            .await?;

        drop(http_server); // explicitly keeping it alive until the end
        Ok(())
    }
}
