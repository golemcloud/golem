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
pub mod wasi_host;
pub mod worker;
pub mod workerctx;

use async_trait::async_trait;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_server::WorkerExecutorServer;
use prometheus::Registry;
use std::sync::Arc;
use tokio::runtime::Handle;
use tonic::transport::Server;
use tracing::info;
use wasmtime::component::Linker;
use wasmtime::{Config, Engine};

use crate::grpc::WorkerExecutorImpl;
use crate::http_server::HttpServerImpl;
use crate::services::active_workers::ActiveWorkers;
use crate::services::blob_store::BlobStoreService;
use crate::services::golem_config::{GolemConfig, WorkersServiceConfig};
use crate::services::invocation_key::{InvocationKeyService, InvocationKeyServiceDefault};
use crate::services::key_value::KeyValueService;
use crate::services::oplog::{OplogService, OplogServiceDefault};
use crate::services::promise::PromiseService;
use crate::services::scheduler::{SchedulerService, SchedulerServiceDefault};
use crate::services::shard::{ShardService, ShardServiceDefault};
use crate::services::shard_manager::ShardManagerService;
use crate::services::template::TemplateService;
use crate::services::worker::{WorkerService, WorkerServiceInMemory, WorkerServiceRedis};
use crate::services::worker_activator::{LazyWorkerActivator, WorkerActivator};
use crate::services::worker_enumeration::{
    RunningWorkerEnumerationService, RunningWorkerEnumerationServiceDefault,
    WorkerEnumerationService, WorkerEnumerationServiceInMemory, WorkerEnumerationServiceRedis,
};
use crate::services::{blob_store, key_value, promise, shard_manager, template, All};
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
        template_service: Arc<dyn TemplateService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
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

        info!("Using Redis at {}", golem_config.redis.url());
        let pool = golem_common::redis::RedisPool::configured(&golem_config.redis).await?;

        let template_service = template::configured(
            &golem_config.template_service,
            &golem_config.template_cache,
            &golem_config.compiled_template_service,
        )
        .await;

        let golem_config = Arc::new(golem_config.clone());
        let promise_service = promise::configured(&golem_config.promises, pool.clone());
        let shard_service = Arc::new(ShardServiceDefault::new());
        let lazy_worker_activator = Arc::new(LazyWorkerActivator::new());

        let oplog_service = Arc::new(OplogServiceDefault::new(pool.clone()).await);

        let (worker_service, worker_enumeration_service) = match &golem_config.workers {
            WorkersServiceConfig::InMemory => {
                let worker_service = Arc::new(WorkerServiceInMemory::new());
                let enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync> = Arc::new(
                    WorkerEnumerationServiceInMemory::new(worker_service.clone()),
                );

                (
                    worker_service as Arc<dyn WorkerService + Send + Sync>,
                    enumeration_service,
                )
            }
            WorkersServiceConfig::Redis => {
                let worker_service =
                    Arc::new(WorkerServiceRedis::new(pool.clone(), shard_service.clone()));
                let enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync> =
                    Arc::new(WorkerEnumerationServiceRedis::new(
                        pool.clone(),
                        worker_service.clone(),
                        oplog_service.clone(),
                        golem_config.clone(),
                    ));

                (
                    worker_service as Arc<dyn WorkerService + Send + Sync>,
                    enumeration_service,
                )
            }
        };

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

        let invocation_key_service = Arc::new(InvocationKeyServiceDefault::new(
            golem_config.invocation_keys.pending_key_retention,
            golem_config.invocation_keys.confirm_queue_capacity,
        ));

        let key_value_service =
            key_value::configured(&golem_config.key_value_service, pool.clone());

        let blob_store_service = blob_store::configured(&golem_config.blob_store_service).await;

        let scheduler_service = SchedulerServiceDefault::new(
            pool.clone(),
            shard_service.clone(),
            promise_service.clone(),
            lazy_worker_activator.clone(),
            golem_config.scheduler.refresh_interval,
        );

        let services = self
            .create_services(
                active_workers,
                engine,
                linker,
                runtime.clone(),
                template_service,
                shard_manager_service,
                worker_service,
                worker_enumeration_service,
                running_worker_enumeration_service,
                promise_service,
                golem_config.clone(),
                invocation_key_service,
                shard_service,
                key_value_service,
                blob_store_service,
                lazy_worker_activator.clone(),
                oplog_service,
                scheduler_service,
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
